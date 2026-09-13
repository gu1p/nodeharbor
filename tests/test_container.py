import importlib.util
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts'))
spec=importlib.util.spec_from_file_location('container_build',ROOT/'scripts/container.py')
container=importlib.util.module_from_spec(spec)
spec.loader.exec_module(container)
import release

def fixture(root,malicious=False):
    for target,(os_name,arch,extensions) in release.TARGETS.items():
        prefix=f'nodeharbor-v0.1.20-{target}';assets=[]
        for ext in extensions:
            name=prefix+ext;(root/name).write_bytes(b'package');assets.append({'name':name,'sha256':release.checksum(root/name)})
        if os_name=='linux':
            name=prefix+'-tools.tar.gz'
            with tarfile.open(root/name,'w:gz') as archive:
                for member in ['nodeharbor-agent','nodeharbor-controller','nodeharbor-probe','ui/index.html']:
                    data=(b'\x7fELF\x02\x01\x01'+b'\0'*9+b'\x03\0'+(62 if arch=='amd64' else 183).to_bytes(2,'little')) if member.startswith('nodeharbor') else b'<html>Fleet</html>'
                    info=tarfile.TarInfo(member);info.size=len(data);info.mode=0o755;archive.addfile(info,io.BytesIO(data))
                if malicious:
                    info=tarfile.TarInfo('../../escape');info.size=1;archive.addfile(info,io.BytesIO(b'x'))
            assets.append({'name':name,'sha256':release.checksum(root/name)})
        (root/(prefix+'.json')).write_text(json.dumps({'version':'0.1.20','commit':'a'*40,'target':target,'os':os_name,'arch':arch,'assets':assets}))

class ControllerContainer(unittest.TestCase):
    def test_only_verified_complete_native_releases_can_supply_container_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);dist=root/'dist';dist.mkdir();fixture(dist)
            context=root/'context';container.prepare(dist,'0.1.20','a'*40,context)
            for arch in ['amd64','arm64']:
                self.assertTrue((context/arch/'nodeharbor-controller').is_file())
                self.assertTrue((context/arch/'nodeharbor-probe').is_file())
                self.assertTrue((context/arch/'ui/index.html').is_file())
            self.assertFalse((context/'amd64/nodeharbor-agent').exists())
    def test_path_traversal_is_rejected_before_any_package_is_extracted(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);dist=root/'dist';dist.mkdir();fixture(dist,True)
            with self.assertRaises(ValueError):container.prepare(dist,'0.1.20','a'*40,root/'context')
            self.assertFalse((root/'escape').exists())
    def test_an_existing_image_is_reused_only_for_the_same_source_and_both_architectures(self):
        index={'annotations':{'org.opencontainers.image.revision':'a'*40,'org.opencontainers.image.version':'0.1.20'},'manifests':[{'platform':{'os':'linux','architecture':arch}} for arch in ['amd64','arm64']]}
        self.assertTrue(container.matches(index,'0.1.20','a'*40))
        self.assertFalse(container.matches(index,'0.1.20','b'*40))
        index['manifests'].pop();self.assertFalse(container.matches(index,'0.1.20','a'*40))

    def test_container_builds_use_a_pinned_builder_with_the_gitlab_attestation_fix(self):
        calls=[]
        with patch.object(sys,'argv',['container.py','unused','0.1.20','a'*40]), patch.object(container,'prepare'), patch.object(container,'run',side_effect=lambda args,**kwargs:calls.append(args)), patch.object(container.subprocess,'check_output',return_value='0.1.20 ('+'a'*40+')'):
            container.main()
        create=next(call for call in calls if call[:3]==['docker','buildx','create'])
        self.assertIn('--driver-opt',create)
        image=create[create.index('--driver-opt')+1]
        self.assertEqual(image,'image=moby/buildkit:v0.32.1@sha256:c2ffdf4f39cb9d0fd0f63413d9354721cb83868a3f2f29583c3504a0c43a254b')
        self.assertFalse(any('--provenance=false' in call for call in calls),'Keep build attestations')
