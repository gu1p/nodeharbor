#!/usr/bin/env python3
"""Manage only the enrolled guest's journaled, file-backed storage pool.

`apply` is an explicit stopped-worker maintenance operation. Boot may finish its
exact owned journal while the worker is stopped. Readiness never mutates disks;
neither path repairs a missing member or selects a fallback.
"""
import copy
import hashlib
import json
import os
from pathlib import Path, PurePath, PurePosixPath
import re
import subprocess
import sys
import tempfile
import uuid

GIB = 1024 ** 3
POOL_PATH = PurePosixPath('/var/lib/nodeharbor/storage')
CONFIG_PATH = PurePosixPath('/etc/nodeharbor')
HELPER = '/usr/local/lib/nodeharbor/storage_pool.py'
LVM_PARTITION = 'E6D6D379-F507-44C2-A23C-238F2A3DF928'


def run(*command, input=None, allowed=(0,)):
    result = subprocess.run(command, input=input, stdin=subprocess.DEVNULL if input is None else None,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if result.returncode not in allowed:
        raise RuntimeError(f'{command[0]} failed during storage maintenance; inspect the guest service logs')
    return result.stdout


def validate_request(request, owner):
    if not isinstance(request, dict) or request.get('format') != 1:
        raise ValueError('Unsupported storage request format')
    for name in ('deviceId', 'poolId'):
        try:
            if str(uuid.UUID(request[name])) != request[name]:raise ValueError()
        except (KeyError, TypeError, AttributeError, ValueError) as error:
            raise ValueError(f'Invalid storage {name}') from error
    if request['deviceId'] != owner:raise ValueError('Storage request belongs to another device owner')
    generation = request.get('generation')
    if type(generation) is not int or not 0 <= generation < 2 ** 64:
        raise ValueError('Invalid storage generation')
    disks = request.get('disks')
    if not isinstance(disks, list) or not 1 <= len(disks) <= 16:
        raise ValueError('A storage pool requires one to sixteen disks')
    names, devices = set(), set()
    for disk in disks:
        if not isinstance(disk, dict):raise ValueError('Invalid storage disk')
        name, device, allocation = disk.get('id'), disk.get('device'), disk.get('allocationBytes')
        if not isinstance(name, str) or not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_-]{0,10}', name):
            raise ValueError('Storage disk identifiers require at most eleven ASCII characters')
        if not isinstance(device, str) or not re.fullmatch(r'/dev/vd[b-q]', device):
            raise ValueError('Only additional managed virtio disks can be initialized')
        if name in names or device in devices:raise ValueError('Duplicate storage disk selection')
        if type(allocation) is not int or allocation < GIB or allocation > 1048576 * GIB or allocation % GIB:
            raise ValueError('Storage allocation must be a bounded whole number of GiB')
        if type(disk.get('initialize')) is not bool:raise ValueError('Disk initialization must be explicit')
        names.add(name); devices.add(device)


def validate_transition(request, state):
    if state is None:return
    if any(request.get(key) != state.get(key) for key in ('deviceId', 'poolId')):
        raise ValueError('Storage belongs to a different owner or pool')
    if request['generation'] < state['generation']:raise ValueError('Stale storage generation')
    requested = {disk['id']: disk for disk in request['disks']}
    old = {disk['id']: disk for disk in state['disks']}
    if not old.keys() <= requested.keys():raise ValueError('An existing storage member cannot be removed')
    for name, disk in old.items():
        if requested[name]['initialize']:raise ValueError('An existing disk cannot be reinitialized')
        if requested[name]['allocationBytes'] < disk['allocationBytes']:
            raise ValueError('An existing storage allocation cannot shrink')
    if request['generation'] == state['generation']:
        if requested.keys() != old.keys() or any(requested[name]['allocationBytes'] != old[name]['allocationBytes'] for name in old):
            raise ValueError('A changed storage request requires a new generation')


def plan_devices(request, state, devices):
    validate_request(request, request['deviceId']); validate_transition(request, state)
    old = {disk['id']: disk for disk in state['disks']} if state else {}
    result = []
    used = set()
    for disk in request['disks']:
        member = old.get(disk['id'])
        if member:
            matches = [(device, partition) for device in devices for partition in device['partitions']
                       if partition.get('partitionUuid') == member['partitionUuid']]
            if len(matches) != 1:raise ValueError(f"Storage member {disk['id']} is missing or duplicated")
            device, partition = matches[0]
            if partition.get('pvUuid') != member['pvUuid'] or partition.get('filesystem') != 'LVM2_member':
                raise ValueError(f"Storage member {disk['id']} has changed identity")
        else:
            matches = [device for device in devices if device['path'] == disk['device']]
            if len(matches) != 1:raise ValueError(f"Selected storage disk {disk['id']} is unavailable")
            device = matches[0]; partition = None
            if not disk['initialize']:raise ValueError('A fresh disk requires explicit initialization')
            if device['signatures'] or device['partitions'] or device['mountpoints']:
                raise ValueError('Refusing to initialize a disk containing existing files, partitions, or mounts')
        if device['type'] != 'disk' or device['containsRoot'] or device['path'] in used:
            raise ValueError('Refusing to use a root, duplicate, or non-disk device')
        if device['sizeBytes'] < disk['allocationBytes']:raise ValueError('The selected disk is smaller than its allocation')
        used.add(device['path'])
        result.append({'id': disk['id'], 'device': device['path'],
                       'partition': partition['path'] if partition else device['path'] + '1',
                       'initialize': member is None,
                       'grow': bool(member and disk['allocationBytes'] > member['allocationBytes'])})
    return result


def validate_mount(state, mount):
    if not mount or mount.get('target') != str(POOL_PATH) or mount.get('fstype') != 'ext4':
        raise ValueError('The owned ext4 storage pool is not mounted at its configured location')
    if mount.get('uuid') != state['filesystemUuid'] or 'rw' not in mount.get('options', '').split(','):
        raise ValueError('The mounted storage pool has changed identity or is read-only')


def path_at(root, absolute):
    relative = absolute.as_posix() if isinstance(absolute, PurePath) else str(absolute)
    base = Path(root).resolve(); path = base / relative.lstrip('/')
    if path.resolve() != path:raise ValueError('Managed storage files cannot follow symbolic links')
    return path


def config_path(root, name):
    return path_at(root, CONFIG_PATH / name)


def load_json(path):
    data = path.read_bytes()
    if len(data) > 1024 * 1024:raise ValueError('Storage metadata exceeds its supported size')
    return json.loads(data)


def write_json(path, value):
    write_file(path, json.dumps(value, sort_keys=True, indent=2) + '\n')


def write_file(path, content, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    name = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix='.storage-', delete=False) as temporary:
            name = temporary.name; temporary.write(content.encode()); temporary.flush(); os.fsync(temporary.fileno())
        os.chmod(name, mode); os.replace(name, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:os.fsync(directory)
        finally:os.close(directory)
    finally:
        if name is not None:Path(name).unlink(missing_ok=True)


def owner(root):
    value = config_path(root, 'device-id').read_text().strip()
    uuid.UUID(value)
    return value


def load_state(root):
    state = load_json(config_path(root, 'storage-state.json'))
    validate_request(state, owner(root))
    for disk in state['disks']:
        uuid.UUID(disk['partitionUuid'])
        if not disk.get('pvUuid'):raise ValueError('Storage member has no recorded physical-volume identity')
    uuid.UUID(state['filesystemUuid'])
    return state


def report(execute, program, section, columns):
    value = json.loads(execute(program, '--reportformat', 'json', '-o', columns))
    return [{key: entry[key].strip() if isinstance(entry[key], str) else entry[key] for key in entry}
            for group in value['report'] for entry in group.get(section, [])]


def signatures(execute, device):
    return [entry['type'] for entry in json.loads(execute('wipefs', '--json', '--no-act', device)).get('signatures', [])]


def inspect_devices(execute=run):
    tree = json.loads(execute('lsblk', '--json', '--bytes', '--paths', '--tree', '--output',
                             'PATH,TYPE,SIZE,FSTYPE,UUID,PARTUUID,MOUNTPOINTS'))['blockdevices']
    pvs = {entry['pv_name']: entry for entry in report(execute, 'pvs', 'pv', 'pv_name,pv_uuid,vg_name,vg_uuid')}
    def descendants(node):
        yield node
        for child in node.get('children', []):yield from descendants(child)
    devices = []
    for device in tree:
        if device['type'] != 'disk':continue
        children = list(descendants(device))
        partitions = []
        for child in children:
            if child['type'] != 'part':continue
            pv = pvs.get(child['path'], {})
            partitions.append({'path': child['path'], 'partitionUuid': child.get('partuuid'),
                               'pvUuid': pv.get('pv_uuid'), 'filesystem': child.get('fstype'),
                               'mountpoints': [point for point in child.get('mountpoints', []) if point]})
        devices.append({'path': device['path'], 'type': device['type'], 'sizeBytes': int(device['size']),
                        'containsRoot': any('/' in (child.get('mountpoints') or []) for child in children),
                        'mountpoints': [point for point in device.get('mountpoints', []) if point],
                        'signatures': signatures(execute, device['path']), 'partitions': partitions})
    return devices


def names(state):
    return 'nh_' + uuid.UUID(state['poolId']).hex, 'worker'


def tags(state):
    return {'nh-owner-' + uuid.UUID(state['deviceId']).hex, 'nh-pool-' + uuid.UUID(state['poolId']).hex}


def logical_volumes(execute, vg_name):
    # Requesting segtype makes lvs emit one row per segment. A linear volume
    # spanning several disks therefore has repeated identical identity rows.
    rows = report(execute, 'lvs', 'lv', 'vg_name,lv_name,lv_uuid,lv_tags,segtype')
    return list({json.dumps(entry, sort_keys=True): entry for entry in rows if entry['vg_name'] == vg_name}.values())


def validate_lvm(state, execute):
    vg_name, lv_name = names(state)
    groups = [entry for entry in report(execute, 'vgs', 'vg', 'vg_name,vg_uuid,vg_tags,vg_free_count') if entry['vg_name'] == vg_name]
    volumes = logical_volumes(execute, vg_name)
    if len(groups) != 1 or not tags(state) <= set(groups[0]['vg_tags'].split(',')):
        raise ValueError('Storage volume group is missing or belongs to another owner')
    if state.get('vgUuid') and groups[0]['vg_uuid'] != state['vgUuid']:
        raise ValueError('Storage volume group identity has changed')
    if len(volumes) != 1 or volumes[0]['lv_name'] != lv_name or volumes[0]['segtype'] != 'linear':
        raise ValueError('Storage requires its unique owned linear logical volume')
    if state.get('lvUuid') and volumes[0]['lv_uuid'] != state['lvUuid']:
        raise ValueError('Storage logical volume identity has changed')
    members = [entry for entry in report(execute, 'pvs', 'pv', 'pv_name,pv_uuid,vg_name') if entry['vg_name'] == vg_name]
    if {entry['pv_uuid'] for entry in members} != {disk['pvUuid'] for disk in state['disks']}:
        raise ValueError('Storage pool membership has changed')
    return groups[0], volumes[0]


def pool_mount(root, execute):
    target = path_at(root, POOL_PATH)
    output = execute('findmnt', '--json', '--mountpoint', str(target), '--output',
                     'TARGET,FSTYPE,UUID,OPTIONS', allowed=(0, 1))
    if not output.strip():return None
    result = json.loads(output)
    mounts = result.get('filesystems', [])
    if len(mounts) != 1:return None
    mount = mounts[0]
    if mount.get('target') == str(target):mount['target'] = str(POOL_PATH)
    return mount


def check_pool(root=Path('/'), execute=run):
    state = load_state(root)
    plan_devices(state, state, inspect_devices(execute))
    validate_mount(state, pool_mount(root, execute))
    validate_lvm(state, execute)
    stats = os.statvfs(path_at(root, POOL_PATH))
    return {**state, 'mountPoint': str(POOL_PATH), 'capacityBytes': stats.f_blocks * stats.f_frsize,
            'availableBytes': stats.f_bavail * stats.f_frsize}


def activate_pool(root=Path('/'), execute=run, recover_pending=True):
    pending_path = config_path(root, 'storage-pending.json')
    if recover_pending and pending_path.exists():
        # A crash after vgextend leaves the old manifest incomplete. Finish only
        # the already-authorized request before requiring its final membership.
        require_stopped(execute)
        pending = load_json(pending_path)
        try:request = {key: pending[key] for key in ('format', 'deviceId', 'poolId', 'generation', 'disks')}
        except KeyError as error:raise ValueError('The pending storage request is incomplete') from error
        validate_request(request, owner(root))
        if fingerprint(request) != pending.get('requestDigest'):
            raise ValueError('The pending storage request digest has changed')
        return apply_request(request, root, execute)
    state = load_state(root)
    plan_devices(state, state, inspect_devices(execute))
    validate_lvm(state, execute)
    vg_name, lv_name = names(state)
    execute('lvchange', '--activate', 'y', '--activationmode', 'complete', f'{vg_name}/{lv_name}')
    mount = pool_mount(root, execute)
    if mount is None:
        target = path_at(root, POOL_PATH); target.mkdir(parents=True, exist_ok=True)
        if any(target.iterdir()):raise ValueError('Refusing to hide existing files below the pool mount')
        execute('mount', '--types', 'ext4', '--source', 'UUID=' + state['filesystemUuid'], '--target', str(target))
    return check_pool(root, execute)


def require_stopped(execute):
    for service in ('k3s-agent', 'k3s'):
        status = execute('systemctl', 'show', service, '--property=ActiveState', '--value').strip()
        if status not in ('inactive', 'failed', ''):raise ValueError('Stop the running Kubernetes worker before storage maintenance')


def migrate_legacy(root=Path('/'), execute=run):
    state = load_state(root); require_stopped(execute); check_pool(root, execute)
    if state.get('migrationComplete'):return state
    sources = [('/var/lib/rancher/k3s', 'k3s'), ('/var/lib/kubelet', 'kubelet'), ('/var/log/pods', 'pods')]
    mounts = json.loads(execute('findmnt', '--json', '--output', 'TARGET')).get('filesystems', [])
    def targets(entries):
        for entry in entries:
            yield entry['target']
            yield from targets(entry.get('children', []))
    mounted = list(targets(mounts))
    for source_name, destination_name in sources:
        source = path_at(root, source_name)
        if any(target == str(source) or target.startswith(str(source) + '/') for target in mounted):
            raise ValueError('Unmount existing worker volumes before migrating storage')
        destination = path_at(root, POOL_PATH / destination_name); destination.mkdir(parents=True, exist_ok=True)
        if not source.exists():continue
        if not source.is_dir():raise ValueError('Legacy storage is not a directory')
        arguments = ['-aHAXS', '--numeric-ids', '--one-file-system']
        execute('rsync', *arguments, str(source) + '/', str(destination) + '/')
        differences = execute('rsync', *arguments, '--checksum', '--dry-run', '--itemize-changes',
                              str(source) + '/', str(destination) + '/')
        if differences.strip():raise ValueError('Legacy storage copy did not pass verification')
    state['migrationComplete'] = True
    write_json(config_path(root, 'storage-state.json'), state)
    return state


def lvm_uuid():
    value = uuid.uuid4().hex
    return '-'.join((value[:6], value[6:10], value[10:14], value[14:18], value[18:22], value[22:26], value[26:]))


def fingerprint(request):
    return hashlib.sha256(json.dumps(request, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def install_pool_service(root, execute):
    unit = f'''[Unit]
Description=Activate the owned NodeHarbor storage pool
After=local-fs.target
Before=k3s-agent.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/bin/python3 {HELPER} activate
[Install]
WantedBy=multi-user.target
'''
    write_file(path_at(root, '/etc/systemd/system/nodeharbor-storage.service'), unit, 0o644)
    execute('systemctl', 'daemon-reload'); execute('systemctl', 'enable', 'nodeharbor-storage.service')


def apply_request(request, root=Path('/'), execute=run):
    validate_request(request, owner(root)); require_stopped(execute)
    state_path = config_path(root, 'storage-state.json')
    pending_path = config_path(root, 'storage-pending.json')
    state = load_state(root) if state_path.exists() else None
    digest = fingerprint(request)
    if pending_path.exists():
        pending = load_json(pending_path)
        if pending.get('requestDigest') != digest or pending.get('deviceId') != owner(root):
            raise ValueError('A different storage operation is pending; resume its original request')
        if state and state.get('appliedRequestDigest') == digest:
            install_pool_service(root, execute)
            result = activate_pool(root, execute, recover_pending=False)
            pending_path.unlink()
            return result
    else:
        if state and state.get('appliedRequestDigest') == digest:
            install_pool_service(root, execute)
            return activate_pool(root, execute, recover_pending=False)
        plans = plan_devices(request, state, inspect_devices(execute))
        old = {disk['id']: disk for disk in state['disks']} if state else {}
        members = []
        for disk, plan in zip(request['disks'], plans):
            member = {**disk, **old.get(disk['id'], {}), 'allocationBytes': disk['allocationBytes'],
                      'device': plan['device'], 'initialize': False}
            if plan['initialize']:
                member.update(diskUuid=str(uuid.uuid4()), partitionUuid=str(uuid.uuid4()), pvUuid=lvm_uuid())
            members.append(member)
        pending = {**copy.deepcopy(request), 'requestDigest': digest, 'oldState': state,
                   'members': members, 'filesystemUuid': state['filesystemUuid'] if state else str(uuid.uuid4())}
        write_json(pending_path, pending)
    state = pending['oldState']
    old = {disk['id']: disk for disk in state['disks']} if state else {}
    partitions = []
    for member in pending['members']:
        devices = inspect_devices(execute)
        matches = [(device, partition) for device in devices for partition in device['partitions']
                   if partition.get('partitionUuid') == member['partitionUuid']]
        if member['id'] in old:
            candidate = {**request, 'disks': [{**member, 'initialize': False}]}
            old_member_state = {**state, 'disks': [old[member['id']]]}
            resolved = plan_devices(candidate, old_member_state, devices)[0]
            device, partition_path = resolved['device'], resolved['partition']
            if member['allocationBytes'] > old[member['id']]['allocationBytes']:
                result = execute('growpart', device, '1', allowed=(0, 1))
                if not result.startswith(('CHANGED:', 'NOCHANGE:')):raise ValueError('Storage partition growth failed')
        else:
            if not matches:
                fresh = {**request, 'disks': [{**member, 'initialize': True}]}
                resolved = plan_devices(fresh, None, devices)[0]
                device = resolved['device']
                script = f"label: gpt\nlabel-id: {member['diskUuid']}\n\ntype={LVM_PARTITION}, uuid={member['partitionUuid']}\n"
                execute('sfdisk', '--wipe', 'never', '--wipe-partitions', 'never', device, input=script)
                execute('udevadm', 'settle')
                matches = [(entry, partition) for entry in inspect_devices(execute) for partition in entry['partitions']
                           if partition.get('partitionUuid') == member['partitionUuid']]
            if len(matches) != 1:raise ValueError('The journaled storage partition is missing or duplicated')
            device_entry, partition = matches[0]
            if device_entry['containsRoot'] or device_entry['type'] != 'disk' or len(device_entry['partitions']) != 1:
                raise ValueError('Journaled initialization cannot adopt a changed disk')
            if device_entry['sizeBytes'] < member['allocationBytes'] or partition['mountpoints']:
                raise ValueError('Journaled storage is undersized or already mounted')
            device, partition_path = device_entry['path'], partition['path']
            if partition.get('pvUuid') != member['pvUuid']:
                if partition.get('pvUuid') or signatures(execute, partition_path):
                    raise ValueError('Refusing to overwrite an unrecognized partition')
                execute('pvcreate', '--yes', '--zero', 'n', '--uuid', member['pvUuid'], '--norestorefile', partition_path)
        current = [entry for entry in report(execute, 'pvs', 'pv', 'pv_name,pv_uuid,vg_name') if entry['pv_name'] == partition_path]
        if len(current) != 1 or current[0]['pv_uuid'] != member['pvUuid']:
            raise ValueError('Physical-volume initialization did not preserve its journaled identity')
        execute('pvresize', partition_path)
        member['device'] = device; partitions.append(partition_path)
    next_state = {key: pending[key] for key in ('format', 'deviceId', 'poolId', 'generation', 'filesystemUuid')}
    next_state.update(disks=pending['members'], migrationComplete=bool(state and state.get('migrationComplete')),
                      appliedRequestDigest=digest)
    vg_name, lv_name = names(next_state)
    groups = [entry for entry in report(execute, 'vgs', 'vg', 'vg_name,vg_uuid,vg_tags,vg_free_count') if entry['vg_name'] == vg_name]
    owner_tags = tags(next_state)
    if not groups:
        if state:raise ValueError('The existing storage volume group is unavailable')
        arguments = [argument for tag in sorted(owner_tags) for argument in ('--addtag', tag)]
        execute('vgcreate', '--setautoactivation', 'n', *arguments, vg_name, *partitions)
    else:
        if len(groups) != 1 or not owner_tags <= set(groups[0]['vg_tags'].split(',')):
            raise ValueError('Refusing to change another storage volume group')
        if state and groups[0]['vg_uuid'] != state['vgUuid']:raise ValueError('Volume-group identity changed')
        all_pvs = report(execute, 'pvs', 'pv', 'pv_name,pv_uuid,vg_name')
        expected = {member['pvUuid'] for member in pending['members']}
        if any(entry['vg_name'] == vg_name and entry['pv_uuid'] not in expected for entry in all_pvs):
            raise ValueError('Storage volume group contains an unexpected disk')
        additional = []
        for entry in all_pvs:
            if entry['pv_uuid'] not in expected:continue
            if entry['vg_name'] and entry['vg_name'] != vg_name:raise ValueError('Storage member belongs to another volume group')
            if not entry['vg_name']:additional.append(entry['pv_name'])
        if additional:execute('vgextend', vg_name, *additional)
    volumes = logical_volumes(execute, vg_name)
    if not volumes:
        if state:raise ValueError('The existing storage logical volume is unavailable')
        arguments = [argument for tag in sorted(owner_tags) for argument in ('--addtag', tag)]
        execute('lvcreate', '--type', 'linear', '--extents', '100%FREE', '--name', lv_name,
                '--setautoactivation', 'n', *arguments, vg_name)
    elif len(volumes) != 1 or volumes[0]['lv_name'] != lv_name or volumes[0]['segtype'] != 'linear' or not owner_tags <= set(volumes[0]['lv_tags'].split(',')):
        raise ValueError('Refusing to change an unrecognized logical volume')
    if state:next_state.update(vgUuid=state['vgUuid'], lvUuid=state['lvUuid'])
    group, volume = validate_lvm(next_state, execute)
    next_state.update(vgUuid=group['vg_uuid'], lvUuid=volume['lv_uuid'])
    if int(group['vg_free_count']) > 0:execute('lvextend', '--extents', '+100%FREE', f'{vg_name}/{lv_name}')
    execute('lvchange', '--activate', 'y', '--activationmode', 'complete', f'{vg_name}/{lv_name}')
    logical = f'/dev/{vg_name}/{lv_name}'
    attributes = dict(line.split('=', 1) for line in execute('blkid', '--probe', '--output', 'export', logical, allowed=(0, 2)).splitlines() if '=' in line)
    if not attributes:
        if state:raise ValueError('The existing storage filesystem is unavailable')
        execute('mkfs.ext4', '-U', next_state['filesystemUuid'], '-m', '0', '-L', 'nodeharbor', logical)
    elif attributes.get('TYPE') != 'ext4' or attributes.get('UUID') != next_state['filesystemUuid']:
        raise ValueError('Refusing to format a changed storage filesystem')
    execute('resize2fs', logical)
    install_pool_service(root, execute)
    write_json(state_path, next_state)
    result = activate_pool(root, execute, recover_pending=False)
    pending_path.unlink()
    return result


def main():
    if sys.platform != 'linux' or os.geteuid() != 0:
        raise SystemExit('Storage maintenance runs only as root inside the owned Linux worker')
    if len(sys.argv) != 2 or sys.argv[1] not in ('apply', 'activate', 'check', 'migrate', 'retire', 'restored'):
        raise SystemExit('Expected apply, activate, check, or migrate')
    import fcntl
    owner(Path('/'))
    lock = config_path(Path('/'), 'storage.lock')
    descriptor = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'r+') as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        if sys.argv[1] in ('retire', 'restored'):
            require_stopped(run)
            request = json.load(sys.stdin)
            state_path = config_path(Path('/'), 'storage-state.json')
            if state_path.exists():
                state = load_state(Path('/'))
                if request.get('poolId') != state['poolId']: raise ValueError('Replacement pool identity changed')
                if sys.argv[1] == 'retire':
                    run('systemctl', 'disable', '--now', 'nodeharbor-storage.service')
                    if pool_mount(Path('/'), run): run('umount', str(POOL_PATH))
                    for name in ('storage-state.json', 'storage-request.json', 'storage-pending.json'):
                        config_path(Path('/'), name).unlink(missing_ok=True)
                else:
                    check_pool(); state['migrationComplete'] = True; write_json(state_path, state)
            elif sys.argv[1] == 'restored': raise ValueError('Replacement storage is unavailable')
            result = {'ok': True}
        elif sys.argv[1] == 'apply':
            contents = sys.stdin.buffer.read(1024 * 1024 + 1)
            if len(contents) > 1024 * 1024:raise ValueError('Storage request exceeds its supported size')
            result = apply_request(json.loads(contents))
        else:result = {'activate': activate_pool, 'check': check_pool, 'migrate': migrate_legacy}[sys.argv[1]]()
        print(json.dumps(result))


if __name__ == '__main__':main()
