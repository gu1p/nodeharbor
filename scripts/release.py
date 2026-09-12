#!/usr/bin/env python3
"""Release versions and verification, shared by native packaging and publication."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import shutil
import tarfile
import tomllib
import os
import urllib.request
import urllib.error

TARGETS = {
    "x86_64-unknown-linux-gnu": ("linux", "amd64", [".deb", ".AppImage"]),
    "aarch64-unknown-linux-gnu": ("linux", "arm64", [".deb", ".AppImage"]),
    "x86_64-apple-darwin": ("macos", "amd64", [".dmg", ".app.tar.gz"]),
    "aarch64-apple-darwin": ("macos", "arm64", [".dmg", ".app.tar.gz"]),
    "x86_64-pc-windows-msvc": ("windows", "amd64", [".exe"]),
}

def version_for(base: str, history_position: int) -> str:
    match = re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", base)
    if not match or history_position < 1:
        raise ValueError("Version requires numeric SemVer and a positive main history position")
    return f"{match[1]}.{match[2]}.{history_position}"

def checksum(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()

def validate_assets(folder: Path, version: str, commit: str) -> list[dict]:
    manifests = []
    seen = set()
    for target, (os_name, arch, extensions) in TARGETS.items():
        file = folder / f"nodeharbor-v{version}-{target}.json"
        if not file.is_file():
            raise ValueError(f"Missing target manifest: {target}")
        manifest = json.loads(file.read_text())
        if (manifest.get("version"), manifest.get("commit"), manifest.get("target")) != (version, commit, target):
            raise ValueError(f"Mismatched release identity: {target}")
        if (manifest.get('os'), manifest.get('arch')) != (os_name, arch):
            raise ValueError(f'Mismatched native platform: {target}')
        assets = manifest.get("assets", [])
        for extension in extensions:
            if not any(asset["name"].endswith(extension) for asset in assets):
                raise ValueError(f"Missing {extension} package for {target}")
        for asset in assets:
            name = asset["name"]
            if name in seen or '\\' in name or Path(name).name != name or not name.startswith(f'nodeharbor-v{version}-{target}') or not (folder / name).is_file():
                raise ValueError("Missing asset or unsafe asset path")
            seen.add(name)
            if checksum(folder / name) != asset["sha256"]:
                raise ValueError(f"Checksum mismatch: {name}")
        manifests.append(manifest)
    return manifests

def stamp(root: Path, version: str, commit: str):
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version) or not re.fullmatch('[0-9a-f]{40}', commit):
        raise ValueError('A numeric version and full source SHA are required')
    cargo=root/'Cargo.toml'
    cargo.write_text(re.sub(r'(\[workspace\.package\][\s\S]*?^version = ")[^"]+(")',lambda m:m[1]+version+m[2],cargo.read_text(),count=1,flags=re.MULTILINE))
    # Only workspace package versions change. Keep all dependency pins intact.
    lock=root/'Cargo.lock';parts=lock.read_text().split('[[package]]')
    for index in range(1,len(parts)):
        if re.search(r'^name = "nodeharbor-[^"]+"$',parts[index],re.MULTILINE):
            parts[index]=re.sub(r'^version = "[^"]+"$',f'version = "{version}"',parts[index],count=1,flags=re.MULTILINE)
    lock.write_text('[[package]]'.join(parts))
    for name in ['desktop/tauri.conf.json','ui/package.json','ui/package-lock.json']:
        path=root/name;data=json.loads(path.read_text());data['version']=version
        if name.endswith('package-lock.json'):data['packages']['']['version']=version
        path.write_text(json.dumps(data,indent=2)+'\n')
    # Validate the edited TOML before invoking any compiler.
    tomllib.loads(cargo.read_text());tomllib.loads(lock.read_text())

def latest_release(releases: list[dict], history: list[str]) -> str | None:
    ranks={sha:index for index,sha in enumerate(history)}
    eligible=[r for r in releases if not r.get('draft') and not r.get('prerelease') and r.get('target_commitish') in ranks]
    return min(eligible,key=lambda r:ranks[r['target_commitish']])['tag_name'] if eligible else None

def publication_action(existing: dict | None, manifest: dict) -> str:
    if existing is None:return 'create'
    if existing.get('target_commitish') != manifest['commit']:
        raise ValueError('This release tag belongs to a different source commit')
    if existing.get('draft'):return 'resume'
    if existing.get('manifest') != manifest:
        raise ValueError('Published release assets are immutable')
    return 'skip'

def github_release(repo: str, tag: str) -> dict | None:
    token=os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if not token:raise ValueError('A GitHub release credential is required')
    request=urllib.request.Request(f'https://api.github.com/repos/{repo}/releases/tags/{tag}',headers={'Authorization':f'Bearer {token}','Accept':'application/vnd.github+json'})
    try:
        with urllib.request.urlopen(request,timeout=30) as response:return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code==404:return None
        raise

def publish(folder: Path, version: str, commit: str, repo: str, key: Path):
    manifests=validate_assets(folder,version,commit)
    manifest={'version':version,'commit':commit,'targets':manifests}
    manifest_file=folder/'release-manifest.json'
    manifest_file.write_text(json.dumps(manifest,indent=2)+'\n')
    tag=f'v{version}'
    existing=github_release(repo,tag)
    if existing and not existing['draft']:
        asset=next((a for a in existing['assets'] if a['name']=='release-manifest.json'),None)
        if not asset:raise ValueError('The published release has no immutable manifest')
        with urllib.request.urlopen(asset['browser_download_url'],timeout=30) as response:existing['manifest']=json.load(response)
    action=publication_action(existing,manifest)
    if action=='skip':
        print(f'{tag} already contains these immutable assets')
        return
    assets=[folder/asset['name'] for target in manifests for asset in target['assets']]
    assets += [folder/f'nodeharbor-v{version}-{target}.json' for target in TARGETS]
    assets.append(manifest_file)
    sums=folder/'SHA256SUMS'
    sums.write_text(''.join(f'{checksum(path)}  {path.name}\n' for path in sorted(assets)))
    for path in [sums,manifest_file]:
        subprocess.run(['minisign','-S','-s',str(key),'-m',str(path),'-t',f'NodeHarbor {tag} source {commit}'],check=True)
        subprocess.run(['minisign','-V','-p','nodeharbor.minisign.pub','-m',str(path)],check=True)
    notes=folder/'release-notes.md'
    notes.write_text(f'NodeHarbor {tag}, built from `{commit}`.\n\nNative packages for macOS (Apple Silicon and Intel), Ubuntu (ARM64 and x64), and Windows x64. All five targets passed the required checks before publication.\n\nVerify downloads using `SHA256SUMS`. The checksum list and release manifest include Minisign signatures verified with the public key in this repository. Pilot desktop packages do not yet carry Apple Developer ID or Windows distribution certificates.\n\nInstalling the app does not enable sharing. See the repository README for prerequisites and fleet setup.\n')
    if action=='create':
        subprocess.run(['gh','release','create',tag,'--repo',repo,'--draft','--target',commit,'--title',f'NodeHarbor {tag}','--notes-file',str(notes)],check=True)
    uploads=assets+[sums,folder/'SHA256SUMS.minisig',folder/'release-manifest.json.minisig',Path('nodeharbor.minisign.pub')]
    subprocess.run(['gh','release','upload',tag,'--repo',repo,'--clobber',*[str(path) for path in uploads]],check=True)
    # Re-read immediately before the transition to avoid editing a published
    # release. Builds for different commits never share a tag.
    current=github_release(repo,tag)
    if not current or not current['draft'] or current['target_commitish']!=commit:
        raise ValueError('Release publication state changed unexpectedly')
    subprocess.run(['gh','release','edit',tag,'--repo',repo,'--draft=false','--latest=false'],check=True)

def collect(root: Path, target_dir: Path, output: Path, target: str, version: str, commit: str):
    os_name,arch,extensions=TARGETS[target]
    output.mkdir(parents=True,exist_ok=True)
    bundle=target_dir/target/'release'/'bundle'
    prefix=f'nodeharbor-v{version}-{target}'
    assets=[]
    for extension in extensions:
        destination=output/f'{prefix}{extension}'
        if extension=='.app.tar.gz':
            sources=list((bundle/'macos').glob('*.app'))
            if len(sources)!=1:raise ValueError('Expected one native macOS application bundle')
            with tarfile.open(destination,'w:gz',format=tarfile.PAX_FORMAT) as archive:archive.add(sources[0],arcname=sources[0].name)
        else:
            sources=[p for p in bundle.rglob(f'*{extension}') if p.is_file()]
            if len(sources)!=1:raise ValueError(f'Expected one {extension} package, found {len(sources)}')
            shutil.copyfile(sources[0],destination)
        assets.append({'name':destination.name,'sha256':checksum(destination)})
    executables=['nodeharbor-agent']+(['nodeharbor-controller'] if os_name=='linux' else [])
    archive_path=output/f'{prefix}-tools.tar.gz'
    with tarfile.open(archive_path,'w:gz',format=tarfile.PAX_FORMAT) as archive:
        for executable in executables:
            name=executable+('.exe' if os_name=='windows' else '')
            path=target_dir/target/'release'/name
            if not path.is_file():raise ValueError(f'Missing worker tool: {name}')
            archive.add(path,arcname=name)
    assets.append({'name':archive_path.name,'sha256':checksum(archive_path)})
    (output/f'{prefix}.json').write_text(json.dumps({'version':version,'commit':commit,'target':target,'os':os_name,'arch':arch,'assets':assets},indent=2)+'\n')

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    version_parser = commands.add_parser("version")
    version_parser.add_argument("--base", default="0.1.0")
    validate_parser = commands.add_parser("validate")
    validate_parser.add_argument("folder", type=Path)
    validate_parser.add_argument("version")
    validate_parser.add_argument("commit")
    stamp_parser=commands.add_parser('stamp')
    stamp_parser.add_argument('version');stamp_parser.add_argument('commit')
    collect_parser=commands.add_parser('collect')
    collect_parser.add_argument('target',choices=TARGETS);collect_parser.add_argument('version');collect_parser.add_argument('commit')
    collect_parser.add_argument('--target-dir',type=Path,default=Path('target'))
    collect_parser.add_argument('--output',type=Path,default=Path('dist'))
    latest_parser=commands.add_parser('reconcile-latest');latest_parser.add_argument('--repo',default='gu1p/nodeharbor')
    publish_parser=commands.add_parser('publish');publish_parser.add_argument('folder',type=Path)
    publish_parser.add_argument('version');publish_parser.add_argument('commit');publish_parser.add_argument('--repo',default='gu1p/nodeharbor');publish_parser.add_argument('--key',required=True,type=Path)
    args = parser.parse_args()
    root=Path(__file__).resolve().parents[1]
    if args.command == "version":
        count = int(subprocess.check_output(["git", "rev-list", "--first-parent", "--count", "HEAD"], text=True))
        print(version_for(args.base, count))
    elif args.command=='stamp':stamp(root,args.version,args.commit)
    elif args.command=='collect':collect(root,args.target_dir,args.output,args.target,args.version,args.commit)
    elif args.command=='publish':publish(args.folder,args.version,args.commit,args.repo,args.key)
    elif args.command=='reconcile-latest':
        pages=json.loads(subprocess.check_output(['gh','api',f'repos/{args.repo}/releases','--paginate','--slurp'],text=True))
        history=subprocess.check_output(['git','rev-list','--first-parent','origin/main'],text=True).splitlines()
        latest=latest_release([r for page in pages for r in page],history)
        if latest:subprocess.run(['gh','release','edit',latest,'--repo',args.repo,'--latest'],check=True)
    else:
        manifests = validate_assets(args.folder, args.version, args.commit)
        assets = [asset for manifest in manifests for asset in manifest["assets"]]
        (args.folder / "SHA256SUMS").write_text("".join(f"{asset['sha256']}  {asset['name']}\n" for asset in sorted(assets, key=lambda a: a['name'])))
        (args.folder / "release-manifest.json").write_text(json.dumps({"version": args.version, "commit": args.commit, "targets": manifests}, indent=2) + "\n")
        print(f"Verified {len(manifests)} native targets and {len(assets)} assets")

if __name__ == "__main__":
    main()
