#!/usr/bin/env python3
"""Build native packages and bundle the exact-version worker CLI as a sidecar."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
from release import TARGETS, collect
from package_smoke import smoke_packages

ROOT=Path(__file__).resolve().parents[1]
def run(command,cwd=ROOT,env=None):
    print('+ '+' '.join(map(str,command)),flush=True)
    subprocess.run(list(map(str,command)),cwd=cwd,env=env,check=True)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('target',choices=TARGETS);parser.add_argument('version');parser.add_argument('commit')
    parser.add_argument('--debug',action='store_true',help='Build a local application bundle for inspection without publishing')
    args=parser.parse_args()
    env={**os.environ,'NODEHARBOR_VERSION':args.version,'NODEHARBOR_COMMIT':args.commit}
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=ROOT,text=True))
    target_dir=Path(metadata['target_directory'])
    profile='debug' if args.debug else 'release'
    build=['cargo','build','--locked','-p','nodeharbor-agent','--target',args.target]
    if not args.debug:build.append('--release')
    run(build,env=env)
    extension='.exe' if args.target.endswith('windows-msvc') else ''
    sidecars=ROOT/'desktop/binaries';sidecars.mkdir(parents=True,exist_ok=True)
    shutil.copy2(target_dir/args.target/profile/f'nodeharbor-agent{extension}',sidecars/f'nodeharbor-agent-{args.target}{extension}')
    config=ROOT/'.local/build-config.json';config.parent.mkdir(parents=True,exist_ok=True)
    config.write_text(json.dumps({'bundle':{'externalBin':['binaries/nodeharbor-agent']}}))
    bundles='app' if args.debug and 'darwin' in args.target else ('app,dmg' if 'darwin' in args.target else ('nsis' if extension else 'deb,appimage'))
    tauri=ROOT/'ui/node_modules/.bin'/('tauri.cmd' if os.name=='nt' else 'tauri')
    command=[tauri,'build','--target',args.target,'--bundles',bundles,'--config',config]
    if args.debug:command.append('--debug')
    run(command,cwd=ROOT/'desktop',env=env)
    if args.debug:return
    if 'linux' in args.target:
        run(['cargo','build','--locked','--release','-p','nodeharbor-controller','--target',args.target],env=env)
    collect(ROOT,target_dir,ROOT/'dist',args.target,args.version,args.commit)
    smoke_packages(ROOT/'dist',args.target,args.version,args.commit)

if __name__=='__main__':main()
