#!/usr/bin/env python3
"""Bundle the pinned, unmodified native Lima distribution into supported apps."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT=Path(__file__).resolve().parents[1]
MANIFEST=json.loads((ROOT/'runtime/lima.json').read_text())

def digest(path):
    with Path(path).open('rb') as file:return hashlib.file_digest(file,'sha256').hexdigest()

def extract_bundle(archive,destination,expected,arch):
    destination=Path(destination)
    if digest(archive)!=expected:raise ValueError('The VM runtime archive failed SHA-256 verification')
    if destination.exists():raise ValueError('VM runtime destination already exists')
    destination.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.vm-runtime-',dir=destination.parent) as temporary:
        stage=Path(temporary)/'lima';stage.mkdir()
        with tarfile.open(archive) as bundle:bundle.extractall(stage,filter='data')
        for path in ['bin/limactl',f'share/lima/lima-guestagent.Linux-{arch}.gz','share/doc/lima/LICENSE']:
            if not (stage/path).is_file():raise ValueError('Incomplete VM runtime: '+path)
        inventory={str(path.relative_to(stage)):digest(path) for path in stage.rglob('*') if path.is_file() and not path.is_symlink()}
        (stage/'nodeharbor-runtime.json').write_text(json.dumps({'version':MANIFEST['version'],'archiveSha256':expected,'files':inventory},indent=2)+'\n')
        stage.rename(destination)
    return destination

def verify_bundle(directory):
    directory=Path(directory)
    record=json.loads((directory/'nodeharbor-runtime.json').read_text())
    if record['version']!=MANIFEST['version']:raise ValueError('The packaged VM runtime has the wrong version')
    if record['archiveSha256'] not in {a['sha256'] for a in MANIFEST['archives'].values()}:raise ValueError('Unrecognized VM runtime archive')
    for name,expected in record['files'].items():
        path=directory/name
        if not path.is_file() or digest(path)!=expected:raise ValueError('The packaged VM component is damaged: '+name)

def bundle_lima(target):
    spec=MANIFEST['archives'][target]
    cache=ROOT/'.local/vm-runtime';cache.mkdir(parents=True,exist_ok=True)
    archive=cache/spec['name']
    if not archive.is_file() or digest(archive)!=spec['sha256']:
        with tempfile.NamedTemporaryFile(dir=cache,delete=False) as temporary:
            pending=Path(temporary.name)
        try:
            url=f"https://github.com/lima-vm/lima/releases/download/v{MANIFEST['version']}/{spec['name']}"
            print('Downloading pinned VM runtime: '+spec['name'],flush=True)
            with urllib.request.urlopen(url,timeout=120) as response,pending.open('wb') as output:shutil.copyfileobj(response,output)
            if digest(pending)!=spec['sha256']:raise ValueError('Downloaded VM runtime failed SHA-256 verification')
            pending.replace(archive)
        finally:pending.unlink(missing_ok=True)
    directory=cache/f"lima-{MANIFEST['version']}-{target}"
    if not directory.exists():extract_bundle(archive,directory,spec['sha256'],spec['guestArch'])
    verify_bundle(directory)
    return directory

def bundle_files(directory):
    root=Path(directory).resolve(strict=True)
    files={}
    def visit(path,ancestors):
        resolved=path.resolve(strict=True)
        if not resolved.is_relative_to(root) or resolved in ancestors:
            raise ValueError('VM runtime links must stay inside the bundle without cycles')
        if path.is_dir():
            for child in sorted(path.iterdir()):visit(child,ancestors|{resolved})
        elif path.is_file():
            files['/usr/libexec/nodeharbor/lima/'+path.relative_to(root).as_posix()]=str(path)
        else:raise ValueError('VM runtime contains an unsupported file type')
    visit(root,set())
    return files

def materialize_bundle(directory):
    source=Path(directory)
    bundle_files(source)  # Validate aliases before following them during copying.
    destination=source.with_name(source.name+'-package')
    with tempfile.TemporaryDirectory(prefix='.package-',dir=source.parent) as temporary:
        stage=Path(temporary)/'lima'
        shutil.copytree(source,stage,symlinks=False)
        if destination.exists():shutil.rmtree(destination)
        stage.rename(destination)
    return destination

def bundle_configuration(target):
    bundle={'externalBin':['binaries/nodeharbor-agent']}
    if 'apple-darwin' in target:
        bundle['resources']={str(bundle_lima(target)):'lima/'}
    if 'linux' in target:
        emulator='qemu-system-arm' if target.startswith('aarch64-') else 'qemu-system-x86'
        firmware='qemu-efi-aarch64' if target.startswith('aarch64-') else 'ovmf'
        # Keep the separately verified tools outside linuxdeploy's usr/lib ELF
        # rewriting. Materialize aliases before Debian's tar writer sees them.
        # One directory mapping also keeps TAURI_CONFIG below Linux's env limit.
        files={'/usr/libexec/nodeharbor/lima':str(materialize_bundle(bundle_lima(target)))}
        bundle['linux']={'deb':{'files':files,'depends':['libwebkit2gtk-4.1-0','libayatana-appindicator3-1','libxss1',emulator,firmware,'qemu-utils','openssh-client','gzip']},'appimage':{'files':files}}
    return bundle

def bundle_environment(target,environ):
    env=dict(environ)
    if 'linux' in target:
        # linuxdeploy otherwise strips the pinned Lima executables in AppImages.
        env['NO_STRIP']='1'
    return env

def check_vm_runtime(application,platform='macos'):
    if platform=='macos':directory=Path(application)/'Contents/Resources/lima'
    elif platform=='linux':directory=Path(application)/'usr/libexec/nodeharbor/lima'
    else:raise ValueError('This platform has no bundled Lima runtime')
    verify_bundle(directory)
    output=subprocess.check_output([str(directory/'bin/limactl'),'--version'],text=True,timeout=30).strip()
    if output!='limactl version '+MANIFEST['version']:raise ValueError('The bundled VM runtime reports the wrong version')
