#!/usr/bin/env python3
"""Apply packaged NodeHarbor guest files through the owner's Multipass session."""
import json
import os
from pathlib import Path
import sys
import tempfile
import uuid

FILES={
    '/usr/local/lib/nodeharbor/configure_worker.py':'0700',
    '/usr/local/lib/nodeharbor/watchdog.py':'0700',
    '/etc/systemd/system/nodeharbor-watchdog.service':'0644',
    '/etc/systemd/system/nodeharbor-watchdog.timer':'0644',
}

def install(payload,root=Path('/')):
    root=root.resolve()
    device=payload['deviceId']
    uuid.UUID(device)
    if (root/'etc/nodeharbor/device-id').read_text().strip()!=device:
        raise ValueError('Refusing to update a VM owned by another device')
    files=payload['files']
    if len(files)!=len(FILES) or {entry['path'] for entry in files}!=set(FILES):
        raise ValueError('The guest update must contain exactly the packaged application files')
    targets=[]
    for entry in files:
        if entry.get('owner')!='root:root' or entry.get('permissions')!=FILES[entry['path']] or not isinstance(entry.get('content'),str):
            raise ValueError('Invalid packaged guest file')
        target=root/entry['path'].lstrip('/')
        if target.is_symlink() or target.resolve()!=target:
            raise ValueError('Guest updates cannot follow symbolic links')
        targets.append((target,entry['content'].encode(),int(entry['permissions'],8)))
    staged=[]
    try:
        for target,content,mode in targets:
            target.parent.mkdir(parents=True,exist_ok=True)
            with tempfile.NamedTemporaryFile(dir=target.parent,prefix='.nodeharbor-update-',delete=False) as temporary:
                staged.append((Path(temporary.name),target))
                temporary.write(content)
                temporary.flush()
                os.fsync(temporary.fileno())
                os.chmod(temporary.name,mode)
        # A running watchdog retains a complete old file. If a later rename fails,
        # preparation stops; retry installs the complete bundle again.
        for temporary,target in staged:os.replace(temporary,target)
    finally:
        for temporary,_ in staged:temporary.unlink(missing_ok=True)

def main():
    if sys.platform!='linux' or os.geteuid()!=0:
        raise SystemExit('Guest updates only run as root inside the managed Linux VM')
    content=sys.stdin.buffer.read(2*1024*1024+1)
    if len(content)>2*1024*1024:raise ValueError('Guest update exceeds the supported size')
    install(json.loads(content))

if __name__=='__main__':main()
