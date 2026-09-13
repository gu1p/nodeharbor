#!/usr/bin/env python3
"""Package verified native controller builds as a non-root multi-platform image."""
import argparse
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tarfile
import tempfile
from release import validate_assets, checksum

ROOT=Path(__file__).resolve().parents[1]
IMAGE='registry.gitlab.com/sikalio/infra/nodeharbor-controller'

def prepare(folder,version,commit,output):
    manifests=validate_assets(folder,version,commit)
    selected=[]
    for manifest in manifests:
        if manifest['os']!='linux':continue
        tools=[a for a in manifest['assets'] if a['name'].endswith('-tools.tar.gz')]
        if len(tools)!=1:raise ValueError('Each Linux build must contain one verified tool archive')
        archive=folder/tools[0]['name']
        with tarfile.open(archive) as package:
            names=[]
            for member in package.getmembers():
                name=PurePosixPath(member.name)
                if name.is_absolute() or '..' in name.parts or '\\' in member.name or not (member.isfile() or member.isdir()):
                    raise ValueError('Unsafe controller archive member')
                if member.name in names:raise ValueError('Duplicate controller archive member')
                names.append(member.name)
            for name in ['nodeharbor-controller','nodeharbor-probe','ui/index.html']:
                if name not in names:raise ValueError(f'Missing controller image input: {name}')
            for binary in ['nodeharbor-controller','nodeharbor-probe']:
                with package.extractfile(binary) as stream:header=stream.read(20)
                machine=62 if manifest['arch']=='amd64' else 183
                if header[:6]!=b'\x7fELF\x02\x01' or int.from_bytes(header[18:20],'little')!=machine:
                    raise ValueError('Controller executable architecture differs from its manifest')
        selected.append((manifest['arch'],archive))
    if len(selected)!=2:raise ValueError('Both native Linux architectures are required')
    if output.exists() and any(output.iterdir()):raise ValueError('Use an empty image build directory')
    output.mkdir(parents=True,exist_ok=True)
    for arch,archive in selected:
        destination=output/arch;destination.mkdir()
        with tarfile.open(archive) as package:
            for member in package.getmembers():
                if member.name in ['nodeharbor-controller','nodeharbor-probe'] or member.name=='ui' or member.name.startswith('ui/'):
                    package.extract(member,path=destination,filter='data')
        for name in ['nodeharbor-controller','nodeharbor-probe']:(destination/name).chmod(0o755)
        for file in (destination/'ui').rglob('*'):file.chmod(0o755 if file.is_dir() else 0o644)
        (destination/'ui').chmod(0o755)
    shutil.copyfile(ROOT/'deploy/Dockerfile',output/'Dockerfile')

def matches(index,version,commit):
    annotations=index.get('annotations',{})
    platforms={(m.get('platform',{}).get('os'),m.get('platform',{}).get('architecture')) for m in index.get('manifests',[]) if m.get('platform',{}).get('os')!='unknown'}
    return annotations.get('org.opencontainers.image.revision')==commit and annotations.get('org.opencontainers.image.version')==version and platforms=={('linux','amd64'),('linux','arm64')}

def run(args,**kwargs):
    print('+ '+' '.join(map(str,args)),flush=True)
    return subprocess.run(list(map(str,args)),check=True,**kwargs)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('folder',type=Path);parser.add_argument('version');parser.add_argument('commit')
    parser.add_argument('--publish',action='store_true');parser.add_argument('--image',default=IMAGE)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='nodeharbor-image-') as directory:
        context=Path(directory)/'context';prepare(args.folder,args.version,args.commit,context)
        tag=f'{args.image}:sha-{args.commit}'
        if args.publish:
            registry=args.image.split('/')[0]
            run(['docker','login',registry,'--username',os.environ['NODEHARBOR_REGISTRY_USERNAME'],'--password-stdin'],input=os.environ['NODEHARBOR_REGISTRY_PASSWORD'].encode())
            existing=subprocess.run(['docker','buildx','imagetools','inspect','--raw',tag],capture_output=True,text=True)
            if existing.returncode==0:
                if not matches(json.loads(existing.stdout),args.version,args.commit):raise ValueError('The existing controller image does not match this source; it will not be overwritten')
                print(f'Preserving the existing immutable controller image {tag}')
                return
            if 'not found' not in existing.stderr.lower() and 'manifest_unknown' not in existing.stderr.lower():
                raise RuntimeError('Cannot verify whether the controller image already exists')
        builder='nodeharbor-'+args.commit[:12]
        run(['docker','buildx','create','--name',builder,'--driver','docker-container','--use'])
        try:
            common=['docker','buildx','build','--build-arg',f'VERSION={args.version}','--build-arg',f'COMMIT={args.commit}']
            local='nodeharbor-controller-smoke:'+args.commit[:12]
            run([*common,'--platform','linux/amd64','--load','--tag',local,context])
            for binary in ['/nodeharbor-controller','/nodeharbor-probe']:
                result=subprocess.check_output(['docker','run','--rm','--read-only','--network','none','--entrypoint',binary,local,'--version'],text=True)
                if args.commit not in result or args.version not in result:raise ValueError('Container executable provenance differs from its native build')
            output=['--push','--tag',tag] if args.publish else ['--output',f'type=oci,dest={directory}/controller.oci']
            run([*common,'--platform','linux/amd64,linux/arm64','--annotation',f'index:org.opencontainers.image.revision={args.commit}','--annotation',f'index:org.opencontainers.image.version={args.version}',*output,context])
            if args.publish:
                index=json.loads(subprocess.check_output(['docker','buildx','imagetools','inspect','--raw',tag],text=True))
                if not matches(index,args.version,args.commit):raise ValueError('Published image manifest does not match both native builds')
                print(f'Published immutable controller image {tag}')
        finally:run(['docker','buildx','rm',builder])

if __name__=='__main__':main()
