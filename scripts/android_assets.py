#!/usr/bin/env python3
"""Stage guest assets only when the native libraries match this source tree."""
import hashlib
import json
from pathlib import Path
import shutil
from android_runtime import verify_android_elf

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with path.open('rb') as source: return hashlib.file_digest(source, 'sha256').hexdigest()


def main():
    native = ROOT / 'android/app/src/main/jniLibs'
    receipt_path = native / 'runtime-build.json'
    if not receipt_path.is_file(): raise ValueError('Build the pinned runtime first: python3 scripts/build_android_runtime.py --bundle-sources')
    receipt = json.loads(receipt_path.read_text())
    if (receipt.get('formatVersion'), receipt.get('abi')) != (1, 'arm64-v8a'):
        raise ValueError('Invalid Android runtime build identity')
    if receipt.get('lockSha256') != digest(ROOT / 'android/runtime/lock.json') or receipt.get('patchSha256') != digest(ROOT / 'android/runtime/qemu-android.patch'):
        raise ValueError('Rebuild the Android runtime: its source pins or patch changed')
    sources = {p.name: digest(p) for p in (ROOT / 'android/runtime/network').iterdir() if p.is_file()}
    if receipt.get('networkSources') != sources: raise ValueError('Rebuild the Android runtime: the network adapter changed')
    if set(receipt.get('libraries', {})) != {'libnodeharbor-network.so', 'libqemu-system-aarch64.so'}:
        raise ValueError('The Android runtime is incomplete')
    for name, expected in receipt['libraries'].items():
        path = native / 'arm64-v8a' / name
        if not path.is_file() or digest(path) != expected: raise ValueError('The Android runtime binary changed after its build')
        verify_android_elf(path)
    destination = ROOT / 'android/app/build/generated/worker-assets'
    destination.mkdir(parents=True, exist_ok=True)
    for source in [ROOT / 'android/runtime/lock.json', receipt_path,
                   *(ROOT / 'guest' / name for name in ['configure_worker.py', 'watchdog.py', 'android_control.py'])]:
        shutil.copyfile(source, destination / source.name)
    (destination / 'runtime-credits.txt').write_text(
        'QEMU 11.1.1 — GPL-2.0 (individual files may use other compatible licenses).\n'
        'GLib 2.88.3 — LGPL-2.1-or-later. libslirp 4.9.4 — BSD-3-Clause.\n'
        'The corresponding runtime source bundle includes upstream licenses, '
        'pinned source archives, NodeHarbor changes and build scripts.\n'
        'Source and releases: https://github.com/gu1p/nodeharbor\n'
        'Ubuntu guest downloads are verified separately during worker preparation.\n')
    print('Staged verified runtime metadata and owned guest programs')


if __name__ == '__main__': main()
