#!/usr/bin/env python3
"""Build the small Linux boot acceptance fixture; never included in release APKs."""
import hashlib
import os
from pathlib import Path
import platform
import shutil
import subprocess
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
KERNEL_URL = 'https://cloud-images.ubuntu.com/noble/20260911/unpacked/noble-server-cloudimg-arm64-vmlinuz-generic'
KERNEL_SHA256 = 'b4567593bda98a0723d9234603ec51d6ec7ee0553e1f36793a9da3956a68183e'

def cpio_entry(name, payload=b'', mode=0o100755, inode=1, device_major=0, device_minor=0):
    filename = name.encode() + b'\0'
    fields = [inode, mode, 0, 0, 1, 0, len(payload), 0, 0, device_major, device_minor, len(filename), 0]
    header = b'070701' + ''.join(f'{field:08x}' for field in fields).encode() + filename
    header += b'\0' * (-len(header) % 4)
    return header + payload + b'\0' * (-len(payload) % 4)

def main():
    directory = ROOT / 'android/app/build/boot-probe-assets/boot-probe'
    directory.mkdir(parents=True, exist_ok=True)
    kernel = directory / 'kernel'
    if not kernel.exists():
        with urllib.request.urlopen(KERNEL_URL, timeout=120) as response, kernel.open('wb') as output:
            shutil.copyfileobj(response, output)
    with kernel.open('rb') as source:
        if hashlib.file_digest(source, 'sha256').hexdigest() != KERNEL_SHA256:
            raise ValueError('The Linux acceptance kernel failed SHA-256 verification')
    host = {'Darwin': 'darwin-x86_64', 'Linux': 'linux-x86_64'}[platform.system()]
    ndk = Path(os.environ['ANDROID_HOME']) / 'ndk/28.2.13676358/toolchains/llvm/prebuilt' / host / 'bin'
    binary = directory / 'init'
    subprocess.run([str(ndk / 'aarch64-linux-android33-clang'), '-nostdlib', '-static', '-fno-stack-protector',
                    '-Wl,-e,_start', '-Wl,--build-id=none', '-O2', str(ROOT / 'android/runtime/probe_init.c'), '-o', str(binary)], check=True)
    archive = cpio_entry('dev', mode=0o40755) + cpio_entry('dev/console', mode=0o20600, inode=2, device_major=5, device_minor=1)
    archive += cpio_entry('init', binary.read_bytes(), inode=3) + cpio_entry('TRAILER!!!', mode=0, inode=4)
    (directory / 'initrd').write_bytes(archive)
    binary.unlink()
    print('Prepared the verified isolated Linux boot fixture')

if __name__ == '__main__':
    main()
