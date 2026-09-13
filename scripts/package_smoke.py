#!/usr/bin/env python3
"""Exercise native release packages without enrolling or starting a worker."""
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile


def check_executable(binary, version, commit):
    reported = subprocess.check_output([str(binary), '--version'], text=True, timeout=30).strip()
    if not re.fullmatch(r'[^\r\n]+ ' + re.escape(version) + r' \(' + re.escape(commit) + r'\)', reported):
        raise ValueError('Packaged executable version or source commit is incorrect: ' + str(binary))


def run(args, **kwargs):
    print('Package check: ' + (args if isinstance(args, str) else ' '.join(map(str, args))), flush=True)
    return subprocess.run(args, check=True, timeout=180, **kwargs)


def smoke_packages(folder, target, version, commit):
    folder = Path(folder).resolve()
    prefix = f'nodeharbor-v{version}-{target}'
    with tempfile.TemporaryDirectory(prefix='nodeharbor-package-check-') as temporary:
        root = Path(temporary)
        if 'apple-darwin' in target:
            with tarfile.open(folder / (prefix + '.app.tar.gz')) as archive:
                archive.extractall(root, filter='data')
            app = root / 'NodeHarbor.app/Contents/MacOS'
            for binary in ['nodeharbor', 'nodeharbor-agent']:
                check_executable(app / binary, version, commit)
            mount = root / 'dmg'
            mount.mkdir()
            run(['hdiutil', 'attach', str(folder / (prefix + '.dmg')), '-nobrowse', '-readonly', '-mountpoint', str(mount)], stdout=subprocess.DEVNULL)
            try:
                for binary in ['nodeharbor', 'nodeharbor-agent']:
                    check_executable(mount / 'NodeHarbor.app/Contents/MacOS' / binary, version, commit)
            finally:
                run(['hdiutil', 'detach', str(mount)], stdout=subprocess.DEVNULL)
        elif 'linux' in target:
            deb = root / 'deb'
            run(['dpkg-deb', '--extract', str(folder / (prefix + '.deb')), str(deb)])
            for binary in ['nodeharbor', 'nodeharbor-agent']:
                check_executable(deb / 'usr/bin' / binary, version, commit)
            appimage = folder / (prefix + '.AppImage')
            appimage.chmod(appimage.stat().st_mode | 0o111)
            run([str(appimage), '--appimage-extract'], cwd=root, stdout=subprocess.DEVNULL)
            check_executable(root / 'squashfs-root/AppRun', version, commit)
            check_executable(root / 'squashfs-root/usr/bin/nodeharbor-agent', version, commit)
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
