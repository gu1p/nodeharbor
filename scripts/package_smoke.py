#!/usr/bin/env python3
"""Exercise native release packages without enrolling or starting a worker."""
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import importlib.util
import plistlib
from contextlib import contextmanager


def check_vm_runtime(application,platform='macos'):
    spec=importlib.util.spec_from_file_location('vm_runtime',Path(__file__).with_name('vm_runtime.py'))
    runtime=importlib.util.module_from_spec(spec);spec.loader.exec_module(runtime)
    runtime.check_vm_runtime(application,platform=platform)


def check_executable(binary, version, commit):
    reported = subprocess.check_output([str(binary), '--version'], text=True, timeout=30).strip()
    if not re.fullmatch(r'[^\r\n]+ ' + re.escape(version) + r' \(' + re.escape(commit) + r'\)', reported):
        raise ValueError('Packaged executable version or source commit is incorrect: ' + str(binary))


def run(args, **kwargs):
    print('Package check: ' + (args if isinstance(args, str) else ' '.join(map(str, args))), flush=True)
    return subprocess.run(args, check=True, timeout=180, **kwargs)


def check_macos_bundle(application, version, commit):
    """Verify the complete bundle before executing any packaged component."""
    app = Path(application)
    codesign = '/usr/bin/codesign'
    run([codesign, '--verify', '--deep', '--strict', '--verbose=2', str(app)])
    with (app / 'Contents/Info.plist').open('rb') as source:
        info = plistlib.load(source)
    identifier = 'io.github.gu1p.nodeharbor'
    if (info.get('CFBundleIdentifier') != identifier or info.get('CFBundleExecutable') != 'nodeharbor'
            or info.get('CFBundleShortVersionString') != version or info.get('CFBundleVersion') != version):
        raise ValueError('The macOS bundle identity or version is incorrect')
    description = info.get('NSRemovableVolumesUsageDescription')
    if not isinstance(description, str) or not description.strip():
        raise ValueError('The macOS bundle must explain its removable-volume access')
    signed = subprocess.check_output([codesign, '--display', '--verbose=2', str(app)],
                                     stderr=subprocess.STDOUT, timeout=30).decode()
    if f'Identifier={identifier}' not in signed.splitlines():
        raise ValueError('The macOS signing identity does not match the application bundle')
    binaries = [app / 'Contents/MacOS' / name for name in ['nodeharbor', 'nodeharbor-agent']]
    lima = app / 'Contents/Resources/lima/bin/limactl'
    for binary in [*binaries, lima]:
        if not binary.is_file():
            raise ValueError('The macOS bundle is missing a required executable: ' + binary.name)
    # Lima lives in Resources and retains the upstream signature and entitlements.
    run([codesign, '--verify', '--strict', '--verbose=2', str(lima)])
    entitlements = plistlib.loads(subprocess.check_output(
        [codesign, '--display', '--entitlements', '-', '--xml', str(lima)],
        stderr=subprocess.DEVNULL, timeout=30))
    for name in ['virtualization', 'network.client', 'network.server']:
        if entitlements.get(f'com.apple.security.{name}') is not True:
            raise ValueError('The bundled Lima runtime is missing its ' + name + ' entitlement')
    for binary in binaries:
        check_executable(binary, version, commit)
    check_vm_runtime(app)


@contextmanager
def mounted_image(path):
    output=subprocess.check_output(['hdiutil','attach',str(path),'-nobrowse','-readonly','-plist'],timeout=180)
    entities=plistlib.loads(output).get('system-entities',[])
    device=next((entry.get('dev-entry') for entry in entities if isinstance(entry.get('dev-entry'),str) and entry['dev-entry'].startswith('/dev/disk')),None)
    if device is None:raise ValueError('macOS returned no disk-image device identifier')
    try:
        mounts=[entry['mount-point'] for entry in entities if isinstance(entry.get('mount-point'),str) and entry['mount-point']]
        if len(mounts)!=1:raise ValueError('The installer must mount exactly one volume')
        yield Path(mounts[0])
    finally:
        run(['hdiutil','detach',device],stdout=subprocess.DEVNULL)


def smoke_packages(folder, target, version, commit):
    folder = Path(folder).resolve()
    prefix = f'nodeharbor-v{version}-{target}'
    with tempfile.TemporaryDirectory(prefix='nodeharbor-package-check-') as temporary:
        root = Path(temporary)
        if 'apple-darwin' in target:
            with tarfile.open(folder / (prefix + '.app.tar.gz')) as archive:
                archive.extractall(root, filter='data')
            check_macos_bundle(root / 'NodeHarbor.app', version, commit)
            with mounted_image(folder / (prefix + '.dmg')) as mount:
                check_macos_bundle(mount / 'NodeHarbor.app', version, commit)
        elif 'linux' in target:
            deb = root / 'deb'
            run(['dpkg-deb', '--extract', str(folder / (prefix + '.deb')), str(deb)])
            for binary in ['nodeharbor', 'nodeharbor-agent']:
                check_executable(deb / 'usr/bin' / binary, version, commit)
            check_vm_runtime(deb,platform='linux')
            appimage = folder / (prefix + '.AppImage')
            appimage.chmod(appimage.stat().st_mode | 0o111)
            run([str(appimage), '--appimage-extract'], cwd=root, stdout=subprocess.DEVNULL)
            check_executable(root / 'squashfs-root/AppRun', version, commit)
            check_executable(root / 'squashfs-root/usr/bin/nodeharbor-agent', version, commit)
            check_vm_runtime(root/'squashfs-root',platform='linux')
        elif target == 'x86_64-pc-windows-msvc' and os.name == 'nt':
            install = root / 'NodeHarbor'
            package = folder / (prefix + '.exe')
            # NSIS requires /D as the last unquoted argument. These paths are
            # generated locally; no shell is involved in this CreateProcess call.
            run(f'"{package}" /S /D={install}')
            try:
                for binary in ['nodeharbor.exe', 'nodeharbor-agent.exe']:
                    check_executable(install / binary, version, commit)
            finally:
                uninstall = install / 'uninstall.exe'
                if uninstall.exists():
                    run(f'"{uninstall}" /S _?={install}')
        else:
            raise ValueError('Package smoke checks require the matching native build host')
    print('Native package smoke checks passed for ' + target, flush=True)
