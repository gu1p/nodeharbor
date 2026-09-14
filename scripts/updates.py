#!/usr/bin/env python3
"""Publish the signed Tauri update channel separately from release downloads."""
import argparse
import base64
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.request

from release import validate_assets, github_release, verify_published_installers

ROOT = Path(__file__).resolve().parents[1]
PUBLIC_KEY = ROOT / 'nodeharbor.minisign.pub'
CHANNEL = 'https://gu1p.github.io/nodeharbor/updates'
REPOSITORY = 'gu1p/nodeharbor'

def sign(path: Path, key: Path) -> str:
    with tempfile.TemporaryDirectory() as directory:
        signature = Path(directory) / 'payload.minisig'
        subprocess.run(['minisign', '-S', '-s', str(key), '-m', str(path), '-x', str(signature)], check=True, capture_output=True)
        subprocess.run(['minisign', '-V', '-p', str(PUBLIC_KEY), '-m', str(path), '-x', str(signature)], check=True, capture_output=True)
        return base64.b64encode(signature.read_bytes()).decode('ascii')

def version(value: str) -> tuple[int, int, int]:
    if not isinstance(value, str) or not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', value):
        raise ValueError('The update channel requires a numeric release version')
    return tuple(map(int, value.split('.')))

def should_publish(candidate: str, current: dict | None) -> bool:
    return current is None or version(candidate) > version(current.get('version'))

def current_release() -> dict | None:
    try:
        request = urllib.request.Request(f'{CHANNEL}/latest.json', headers={'Cache-Control': 'no-cache'})
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise

def build(folder: Path, output: Path, release_version: str, commit: str, key: Path):
    manifests = validate_assets(folder, release_version, commit)
    platforms = {}
    for target in manifests:
        os_name = {'macos': 'darwin', 'linux': 'linux', 'windows': 'windows', 'android': 'android'}[target['os']]
        arch = {'arm64': 'aarch64', 'amd64': 'x86_64'}[target['arch']]
        for asset in target['assets']:
            name = asset['name']
            extension = next((suffix for suffix in ['.app.tar.gz', '.AppImage', '.deb', '.exe', '.apk'] if name.endswith(suffix)), None)
            if extension is None:
                continue
            installer = {'.app.tar.gz': 'app', '.AppImage': 'appimage', '.deb': 'deb', '.exe': 'nsis', '.apk': 'apk'}[extension]
            signature = sign(folder / name, key)
            if installer == 'app':
                destination = output / 'updates' / f'v{release_version}' / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(folder / name, destination)
                url = f'{CHANNEL}/v{release_version}/{name}'
            else:
                url = f'https://github.com/{REPOSITORY}/releases/download/v{release_version}/{name}'
            platforms[f'{os_name}-{arch}-{installer}'] = {'url': url, 'signature': signature}
    if len(platforms) != 8:
        raise ValueError('Every supported installer format must have a signed update')
    manifest = {'version': release_version, 'commit': commit,
                'notes': f'NodeHarbor {release_version}. Your sharing rules and fleet enrollment are preserved.',
                'pub_date': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'), 'platforms': platforms}
    (output / 'updates').mkdir(parents=True, exist_ok=True)
    (output / 'updates/latest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (output / 'index.html').write_text('<!doctype html><html lang="en"><meta charset="utf-8"><title>NodeHarbor updates</title><h1>NodeHarbor updates</h1><p>This channel supplies signed updates to NodeHarbor.</p><p><a href="https://github.com/gu1p/nodeharbor/releases/latest">Download an installer</a></p></html>\n')
    (output / '.nojekyll').touch()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('folder', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('version')
    parser.add_argument('commit')
    parser.add_argument('--key', type=Path, required=True)
    parser.add_argument('--github-output', type=Path)
    args = parser.parse_args()
    manifests = validate_assets(args.folder, args.version, args.commit)
    published = github_release(REPOSITORY, f'v{args.version}')
    if not published or published.get('draft') or published.get('prerelease') or published.get('target_commitish') != args.commit:
        raise ValueError('Only the published, fully verified source can enter the update channel')
    verify_published_installers(published, {'targets': manifests})
    deploy = should_publish(args.version, current_release())
    if deploy:
        build(args.folder, args.output, args.version, args.commit, args.key)
    if args.github_output:
        with args.github_output.open('a') as output:
            output.write(f'deploy={str(deploy).lower()}\n')
    print('Signed update channel prepared' if deploy else 'A matching or newer update is already published')

if __name__ == '__main__':
    main()
