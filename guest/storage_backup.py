#!/usr/bin/env python3
"""Versioned, bounded opaque backup stream. Extraction runs only in the owned guest.

Records carry numeric ownership, permissions, nanosecond mtime, hard links and
all extended attributes (including Linux POSIX ACLs). File data is streamed by
allocated extent; holes are restored with truncate/seek rather than zero writes.
"""
import base64
import errno
import json
import os
from pathlib import Path, PurePosixPath
import stat
import struct
import subprocess
import sys

MAGIC = b'NODEHARBOR-BACKUP-1\n'
CHUNK = 1024 * 1024
DEFAULT_ROOTS = ('k3s', 'kubelet', 'pods')


def exact(stream, size):
    result = bytearray()
    while len(result) < size:
        chunk = stream.read(size - len(result))
        if not chunk: raise EOFError('Worker backup is incomplete; preserve the source images')
        result.extend(chunk)
    return bytes(result)


def extents(path, size):
    result = []
    with path.open('rb') as handle:
        offset = 0
        while offset < size:
            try:
                start = os.lseek(handle.fileno(), offset, os.SEEK_DATA)
                end = min(os.lseek(handle.fileno(), start, os.SEEK_HOLE), size)
            except OSError as error:
                if error.errno == errno.ENXIO: break
                if error.errno in (errno.EINVAL, errno.ENOTSUP):
                    result = [[0, size]] if size else []; break
                raise
            if end <= start: raise ValueError('Invalid file extent')
            result.append([start, end - start]); offset = end
    compact = []
    with path.open('rb') as handle:
        for start, length in result:
            handle.seek(start)
            while length:
                count = min(65536, length); data = exact(handle, count)
                if any(data):
                    if compact and compact[-1][0] + compact[-1][1] == start: compact[-1][1] += count
                    else: compact.append([start, count])
                start += count; length -= count
    return compact


def metadata(path, name, links):
    info = path.lstat()
    kind = 'directory' if stat.S_ISDIR(info.st_mode) else 'symlink' if stat.S_ISLNK(info.st_mode) else 'file' if stat.S_ISREG(info.st_mode) else 'fifo' if stat.S_ISFIFO(info.st_mode) else 'character' if stat.S_ISCHR(info.st_mode) else 'block' if stat.S_ISBLK(info.st_mode) else None
    if kind is None: raise ValueError('Remove remaining workload sockets and device mounts before backing up ' + name)
    record = dict(path=name, kind=kind, mode=stat.S_IMODE(info.st_mode), uid=info.st_uid,
                  gid=info.st_gid, mtime=info.st_mtime_ns, xattrs={})
    if hasattr(os, 'listxattr'):
        record['xattrs'] = {key: base64.b64encode(os.getxattr(path, key, follow_symlinks=False)).decode()
                            for key in sorted(os.listxattr(path, follow_symlinks=False))}
    if kind == 'symlink': record['link'] = os.readlink(path)
    if kind in ('character', 'block'): record['device'] = [os.major(info.st_rdev), os.minor(info.st_rdev)]
    if kind != 'directory':
        identity = (info.st_dev, info.st_ino)
        if identity in links:
            record.update(kind='hardlink', link=links[identity])
            if kind == 'symlink': record['symlinkHardlink'] = True
        else:
            links[identity] = name
            if kind == 'file': record.update(size=info.st_size, extents=extents(path, info.st_size))
    return record


def records(root, roots=DEFAULT_ROOTS):
    if roots is None: roots = tuple(sorted(entry.name for entry in root.iterdir()))
    links = {}
    def walk(name):
        path = root / name
        record = metadata(path, name, links)
        yield record
        if record['kind'] == 'directory':
            for entry in sorted(path.iterdir(), key=lambda entry: entry.name):
                yield from walk(name + '/' + entry.name)
    for name in roots:
        if (root / name).exists() or (root / name).is_symlink(): yield from walk(name)


def write_backup(root, output, roots=DEFAULT_ROOTS):
    output.write(MAGIC)
    for record in records(root, roots):
        encoded = json.dumps(record, sort_keys=True, ensure_ascii=True).encode()
        if len(encoded) > CHUNK: raise ValueError('File metadata exceeds the backup limit')
        output.write(struct.pack('>I', len(encoded))); output.write(encoded)
        if record['kind'] == 'file':
            with (root / record['path']).open('rb') as handle:
                for offset, size in record['extents']:
                    handle.seek(offset)
                    while size:
                        data = exact(handle, min(CHUNK, size)); output.write(data); size -= len(data)
    output.write(struct.pack('>I', 0)); output.flush()


def safe_path(root, name, roots):
    path = PurePosixPath(name)
    if not name or name == '.' or path.is_absolute() or '..' in path.parts or str(path) != name or (roots is not None and not any(name == item or name.startswith(item + '/') for item in roots)):
        raise ValueError('Backup contains an unexpected worker path')
    target = root / name
    for parent in target.parents:
        if parent == root: break
        if parent.is_symlink(): raise ValueError('Backup destination contains a symbolic link')
    return target


def read_record(stream):
    size = struct.unpack('>I', exact(stream, 4))[0]
    if size == 0:
        if stream.read(1): raise ValueError('Unexpected trailing backup data')
        return None
    if size > CHUNK: raise ValueError('Invalid backup metadata size')
    record = json.loads(exact(stream, size))
    if record['kind'] not in ('directory', 'file', 'symlink', 'hardlink', 'fifo', 'character', 'block'): raise ValueError('Unsupported backup entry')
    offset = 0
    for start, length in record.get('extents', []):
        if type(start) is not int or type(length) is not int or start < offset or length <= 0 or start + length > record['size']:
            raise ValueError('Invalid sparse backup extent')
        offset = start + length
    return record


def verify_backup(root, stream, roots=DEFAULT_ROOTS):
    if exact(stream, len(MAGIC)) != MAGIC: raise ValueError('Unsupported worker backup')
    actual = iter(records(root, roots))
    while (record := read_record(stream)) is not None:
        observed = next(actual, None)
        if observed is None: raise ValueError('Backup has unexpected files')
        expected_meta = {key: value for key, value in record.items() if key != 'extents'}
        actual_meta = {key: value for key, value in observed.items() if key != 'extents'}
        if expected_meta != actual_meta: raise ValueError('Worker backup metadata verification failed: ' + record['path'])
        if record['kind'] == 'file':
            with safe_path(root, record['path'], roots).open('rb') as handle:
                position = 0
                for start, length in record['extents'] + [[record['size'], 0]]:
                    while position < start:
                        data = exact(handle, min(CHUNK, start - position))
                        if any(data): raise ValueError('Worker backup sparse-hole verification failed')
                        position += len(data)
                    while length:
                        count = min(CHUNK, length)
                        if exact(handle, count) != exact(stream, count): raise ValueError('Worker backup content verification failed')
                        length -= count; position += count
    if next(actual, None) is not None: raise ValueError('Backup is missing worker files')


def apply_metadata(path, record):
    info = path.lstat()
    if (info.st_uid, info.st_gid) != (record['uid'], record['gid']):
        os.chown(path, record['uid'], record['gid'], follow_symlinks=False)
    if not path.is_symlink(): os.chmod(path, record['mode'])
    if hasattr(os, 'listxattr'):
        for key in os.listxattr(path, follow_symlinks=False):
            if key not in record['xattrs']: os.removexattr(path, key, follow_symlinks=False)
        for key, value in record['xattrs'].items():
            os.setxattr(path, key, base64.b64decode(value, validate=True), follow_symlinks=False)
    os.utime(path, ns=(record['mtime'], record['mtime']), follow_symlinks=False)


def restore_backup(root, stream, roots=DEFAULT_ROOTS):
    if exact(stream, len(MAGIC)) != MAGIC: raise ValueError('Unsupported worker backup')
    directories, seen = [], set()
    while (record := read_record(stream)) is not None:
        name = record['path']; target = safe_path(root, name, roots)
        if name in seen: raise ValueError('Duplicate backup destination')
        if target.is_symlink():
            replay_link = record['kind'] == 'symlink' and os.readlink(target) == record['link']
            if record['kind'] == 'hardlink' and record.get('symlinkHardlink') and record['link'] in seen:
                original = safe_path(root, record['link'], roots)
                replay_link = original.lstat().st_ino == target.lstat().st_ino
            if not replay_link:
                raise ValueError('Unexpected symbolic-link backup destination')
            target.unlink()
        if record['kind'] == 'directory':
            target.mkdir(parents=True, exist_ok=True); directories.append((target, record))
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.exists():
                if target.is_dir(): raise ValueError('Unexpected restore destination')
                target.unlink()
            if record['kind'] == 'symlink': target.symlink_to(record['link'])
            elif record['kind'] == 'fifo': os.mkfifo(target, record['mode'])
            elif record['kind'] in ('character', 'block'):
                os.mknod(target, (stat.S_IFCHR if record['kind'] == 'character' else stat.S_IFBLK) | record['mode'], os.makedev(*record['device']))
            elif record['kind'] == 'hardlink':
                if record['link'] not in seen: raise ValueError('Backup hard link has no previous file')
                original = safe_path(root, record['link'], roots)
                if stat.S_ISDIR(original.lstat().st_mode): raise ValueError('Invalid backup hard link')
                os.link(original, target, follow_symlinks=False)
            else:
                with target.open('xb') as handle:
                    handle.truncate(record['size'])
                    for offset, size in record['extents']:
                        handle.seek(offset)
                        while size:
                            data = exact(stream, min(CHUNK, size)); handle.write(data); size -= len(data)
                    handle.flush(); os.fsync(handle.fileno())
            apply_metadata(target, record)
        seen.add(name)
    for path, record in reversed(directories): apply_metadata(path, record)
    if hasattr(os, 'sync'): os.sync()


def layout():
    if Path('/etc/nodeharbor/storage-state.json').exists():
        subprocess.run(['python3', '/usr/local/lib/nodeharbor/storage_pool.py', 'check'], check=True, stdout=subprocess.DEVNULL)
        return Path('/var/lib/nodeharbor/storage'), None
    return Path('/'), ('var/lib/rancher/k3s', 'var/lib/kubelet', 'var/log/pods')


def quiesce():
    # K3s leaves containerd/shims alive when its unit stops. Its supported killall
    # script removes remaining workload mounts inside this owned guest only.
    loaded = subprocess.check_output(['systemctl', 'show', 'k3s-agent.service', '--property=LoadState', '--value'], text=True).strip()
    if loaded == 'loaded': subprocess.run(['systemctl', 'disable', '--now', 'k3s-agent.service'], check=True, stdout=subprocess.DEVNULL)
    killall = Path('/usr/local/bin/k3s-killall.sh')
    if killall.is_file(): subprocess.run([str(killall)], check=True, stdout=subprocess.DEVNULL)
    for program in ('k3s', 'containerd', 'containerd-shim'):
        result = subprocess.run(['pgrep', '-x', program], stdout=subprocess.DEVNULL)
        if result.returncode != 1: raise ValueError('Stop remaining Kubernetes/containerd processes before backup')
    root, roots = layout()
    if roots is None: roots = tuple(entry.name for entry in root.iterdir())
    mounts = subprocess.check_output(['findmnt', '--json', '--output', 'TARGET'], text=True)
    def targets(entries):
        for entry in entries:
            yield entry['target']; yield from targets(entry.get('children', []))
    for mount in sorted(targets(json.loads(mounts).get('filesystems', [])), key=len, reverse=True):
        if any(mount == str(root / item) or mount.startswith(str(root / item) + '/') for item in roots):
            subprocess.run(['umount', '--', mount], check=True, stdout=subprocess.DEVNULL)
    for item in roots:
        for parent, directories, files in os.walk(root / item, followlinks=False):
            for name in files:
                path = Path(parent) / name
                if stat.S_ISSOCK(path.lstat().st_mode): path.unlink()


def main():
    if sys.platform != 'linux' or os.geteuid() != 0: raise ValueError('Backup operations run only inside the owned Linux worker')
    if len(sys.argv) != 3 or Path('/etc/nodeharbor/device-id').read_text().strip() != sys.argv[2]:
        raise ValueError('Worker backup owner does not match')
    action = sys.argv[1]
    if action == 'quiesce': quiesce(); return
    root, roots = layout()
    if action == 'inspect':
        size = 0
        entries = 0
        for name in roots if roots is not None else ('.',):
            if (root / name).exists():
                output = subprocess.check_output(['du', '--summarize', '--one-file-system', '--block-size=1', str(root / name)], text=True)
                size += int(output.split()[0])
                device = (root / name).stat().st_dev
                for parent, directories, files in os.walk(root / name, followlinks=False):
                    directories[:] = [item for item in directories if not (Path(parent) / item).is_symlink() and (Path(parent) / item).stat().st_dev == device]
                    entries += 1 + len(files) + len(directories)
        # Fresh ext4 needs enough inodes as well as enough data blocks.
        print(json.dumps({'dataBytes': max(size, entries * 16384), 'backupBytes': size * 11 // 10 + entries * 4096 + CHUNK})); return
    if action not in ('backup', 'verify', 'restore'): raise ValueError('Unsupported worker backup operation')
    quiesce()
    if action == 'backup': write_backup(root, sys.stdout.buffer, roots)
    elif action == 'verify': verify_backup(root, sys.stdin.buffer, roots)
    else: restore_backup(root, sys.stdin.buffer, roots)


if __name__ == '__main__': main()
