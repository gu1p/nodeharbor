#!/usr/bin/env python3
"""Fixed, replayable storage operations inside the owned Android Linux guest."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid
import storage_backup as backup
import storage_pool as pool

PRIVATE = Path('/var/lib/nodeharbor/android-backups')
CONFIG = Path('/etc/nodeharbor')
GIB = 1024 ** 3


def validate_request(request, owner):
    if not isinstance(request, dict) or request.get('deviceId') != owner or str(uuid.UUID(owner)) != owner:
        raise ValueError('Storage operation belongs to another owner')
    if str(uuid.UUID(request['operation'])) != request['operation']:
        raise ValueError('Invalid storage operation identity')
    if request.get('previousPoolId') is not None and str(uuid.UUID(request['previousPoolId'])) != request['previousPoolId']:
        raise ValueError('Invalid original storage pool identity')
    action = request.get('action')
    if action not in {'inspect', 'backup', 'apply', 'cleanup'}: raise ValueError('Unsupported storage operation')
    fields = {'deviceId', 'operation', 'action', 'previousPoolId'}
    if action == 'apply': fields |= {'pool', 'restore'}
    if set(request) != fields: raise ValueError('Unsupported storage operation fields')
    if action == 'apply':
        pool.validate_request(request['pool'], owner)
        if type(request['restore']) is not bool: raise ValueError('Storage restoration must be explicit')
        if request['restore'] and request['previousPoolId'] is None: raise ValueError('Restoration requires its original pool')
    elif action in {'backup', 'cleanup'} and request['previousPoolId'] is None:
        raise ValueError('A whole-pool backup requires its original pool')


def paths(work, request):
    validate_request(request, request['deviceId'])
    if work.is_symlink(): raise ValueError('Unexpected private backup link')
    work.mkdir(mode=0o700, parents=True, exist_ok=True)
    result = [work / (request['operation'] + extension) for extension in ('.backup', '.json', '.partial')]
    if any(path.is_symlink() for path in result): raise ValueError('Unexpected private backup file link')
    return result


def digest(path):
    with path.open('rb') as source: return hashlib.file_digest(source, 'sha256').hexdigest()


def backup_receipt(work, request):
    _, manifest, _ = paths(work, request)
    with manifest.open('rb') as source:
        data = source.read(65537)
    if len(data) > 65536: raise ValueError('Invalid backup receipt')
    receipt = json.loads(data)
    if (receipt.get('format'), receipt.get('deviceId'), receipt.get('operation'), receipt.get('poolId'), receipt.get('verified')) != (
            1, request['deviceId'], request['operation'], request['previousPoolId'], True):
        raise ValueError('The private backup belongs to a different storage operation')
    return receipt


def verified_receipt(work, request):
    archive, _, partial = paths(work, request)
    receipt = backup_receipt(work, request)
    if not archive.exists() and partial.is_file():
        if partial.stat().st_size != receipt['archiveBytes'] or digest(partial) != receipt['sha256']:
            raise ValueError('The interrupted backup failed verification')
        partial.replace(archive)
    if (not archive.is_file() or archive.stat().st_size != receipt['archiveBytes'] or
            digest(archive) != receipt['sha256']):
        raise ValueError('The private backup failed verification; preserve the original pool')
    return receipt


def estimate(source):
    data_bytes = archive_bytes = entries = 0
    for record in backup.records(source, None):
        entries += 1
        encoded = json.dumps(record, sort_keys=True, ensure_ascii=True).encode()
        archive_bytes += len(encoded) + 4
        if record['kind'] == 'file':
            allocated = sum(length for _, length in record['extents'])
            data_bytes += allocated
            archive_bytes += allocated
    return dict(dataBytes=max(data_bytes, entries * 16384), backupBytes=archive_bytes + len(backup.MAGIC) + 4)


def create_backup(source, work, request, minimum_free=GIB):
    archive, manifest, partial = paths(work, request)
    if manifest.exists():
        receipt = verified_receipt(work, request)
        with archive.open('rb') as stream: backup.verify_backup(source, stream, None)
        return receipt
    if archive.exists(): raise ValueError('An uncommitted private backup must be preserved for recovery')
    amounts = estimate(source)
    space = os.statvfs(work)
    if amounts['backupBytes'] + minimum_free > space.f_bavail * space.f_frsize:
        raise ValueError('The private guest system disk lacks temporary backup capacity; original storage was preserved')
    partial.unlink(missing_ok=True)
    with partial.open('xb') as stream:
        os.chmod(partial, 0o600)
        backup.write_backup(source, stream, None)
        stream.flush(); os.fsync(stream.fileno())
    with partial.open('rb') as stream: backup.verify_backup(source, stream, None)
    receipt = dict(format=1, deviceId=request['deviceId'], operation=request['operation'], poolId=request['previousPoolId'],
                   verified=True, sha256=digest(partial), archiveBytes=partial.stat().st_size, **amounts)
    # Journal before rename; replay can finish exactly this verified partial.
    pool.write_json(manifest, receipt)
    partial.replace(archive)
    return receipt


def restore_verified_backup(target, work, request):
    archive, _, _ = paths(work, request)
    receipt = verified_receipt(work, request)
    if receipt['dataBytes'] + GIB > os.statvfs(target).f_blocks * os.statvfs(target).f_frsize:
        raise ValueError('The replacement storage is too small for the verified backup')
    with archive.open('rb') as stream: backup.restore_backup(target, stream, None)
    with archive.open('rb') as stream: backup.verify_backup(target, stream, None)
    return receipt


def execute(*arguments, input=None):
    result = subprocess.run(arguments, input=input, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=600)
    if result.returncode: raise ValueError('Guest storage maintenance failed; the original disks and backup were preserved')
    return result.stdout


def ensure_tools():
    if any(shutil.which(program) is None for program in ('pvs', 'rsync', 'growpart')):
        environment = dict(os.environ, DEBIAN_FRONTEND='noninteractive')
        for arguments in (['apt-get', 'update'], ['apt-get', 'install', '-y', 'lvm2', 'e2fsprogs', 'util-linux', 'cloud-guest-utils', 'rsync']):
            subprocess.run(arguments, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True, timeout=600)


def stop_worker():
    loaded = execute('systemctl', 'show', 'k3s-agent.service', '--property=LoadState', '--value').decode().strip()
    if loaded == 'loaded': execute('systemctl', 'disable', '--now', 'k3s-agent.service')
    if Path('/usr/local/bin/k3s-killall.sh').is_file(): execute('/usr/local/bin/k3s-killall.sh')
    for program in ('k3s', 'containerd', 'containerd-shim'):
        if subprocess.run(['pgrep', '-x', program], stdout=subprocess.DEVNULL).returncode != 1:
            raise ValueError('Kubernetes must stop before changing its storage')


def operate(request):
    owner = (CONFIG / 'device-id').read_text().strip()
    validate_request(request, owner)
    action = request['action']
    if action in ('inspect', 'backup'):
        state = pool.check_pool()
        if state['poolId'] != request['previousPoolId']: raise ValueError('Original storage pool identity changed')
        source = Path(str(pool.POOL_PATH))
        if action == 'inspect': return estimate(source)
        backup.quiesce()
        return create_backup(source, PRIVATE, request)
    if action == 'cleanup':
        # A current, verified pool must exist before discarding its backup.
        state = pool.check_pool()
        if state['poolId'] == request['previousPoolId']: raise ValueError('The original pool is still active; retain its backup')
        archive, manifest, partial = paths(PRIVATE, request)
        if manifest.exists():
            backup_receipt(PRIVATE, request)
            if archive.exists() or partial.exists(): verified_receipt(PRIVATE, request)
        elif archive.exists() or partial.exists(): raise ValueError('An unrecognized backup file was preserved')
        archive.unlink(missing_ok=True); partial.unlink(missing_ok=True); manifest.unlink(missing_ok=True)
        return {'cleaned': True}
    ensure_tools(); stop_worker()
    desired = request['pool']
    state_path = CONFIG / 'storage-state.json'
    previous = pool.load_state(Path('/')) if state_path.exists() else None
    if request['restore']:
        verified_receipt(PRIVATE, request)
        if previous and previous['poolId'] != desired['poolId']:
            if previous['poolId'] != request['previousPoolId']: raise ValueError('The original pool identity changed')
            execute('python3', '/usr/local/lib/nodeharbor/storage_pool.py', 'retire',
                    input=json.dumps({'poolId': previous['poolId']}).encode())
    elif previous and previous['poolId'] != desired['poolId']:
        raise ValueError('Changing pool identity requires a verified whole-pool backup')
    result = pool.apply_request(desired)
    if request['restore']:
        restore_verified_backup(Path(str(pool.POOL_PATH)), PRIVATE, request)
        execute('python3', '/usr/local/lib/nodeharbor/storage_pool.py', 'restored', input=json.dumps({'poolId': desired['poolId']}).encode())
    else: pool.migrate_legacy()
    result = pool.check_pool()
    if not result.get('migrationComplete') or result['generation'] != desired['generation'] or result['poolId'] != desired['poolId']:
        raise ValueError('Guest storage did not verify the requested generation')
    (CONFIG / 'android-configured-revision').unlink(missing_ok=True)
    return {key: result[key] for key in ('generation', 'poolId', 'capacityBytes', 'availableBytes')}


def main():
    if sys.platform != 'linux' or os.geteuid() != 0: raise ValueError('Storage operations run only inside the owned Linux guest')
    contents = sys.stdin.buffer.read(65537)
    if len(contents) > 65536: raise ValueError('Storage operation exceeds its supported size')
    request = json.loads(contents)
    result = operate(request)
    print(json.dumps(dict(result, operation=request['operation'], action=request['action'], ok=True)))


if __name__ == '__main__': main()
