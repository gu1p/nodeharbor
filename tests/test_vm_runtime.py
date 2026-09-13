"""The macOS installer must include the complete verified VM runtime."""
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
def module():
    spec=importlib.util.spec_from_file_location('vm_runtime',ROOT/'scripts/vm_runtime.py')
    result=importlib.util.module_from_spec(spec);spec.loader.exec_module(result);return result

def archive(path, entries):
    with tarfile.open(path,'w:gz') as output:
        for name,data in entries.items():
            member=tarfile.TarInfo(name);member.size=len(data);member.mode=0o755 if name.startswith('bin/') else 0o644
            output.addfile(member,io.BytesIO(data))
    return hashlib.sha256(path.read_bytes()).hexdigest()

class VmRuntime(unittest.TestCase):
    def test_bundle_keeps_runtime_layout_and_license_in_application_resources(self):
        runtime=module()
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);bundle=root/'runtime.tar.gz';destination=root/'Application Resources/lima'
            digest=archive(bundle,{'bin/limactl':b'native-executable','share/lima/lima-guestagent.Linux-aarch64.gz':b'guest-agent','share/doc/lima/LICENSE':b'Apache License'})
            runtime.extract_bundle(bundle,destination,digest,'aarch64')
            self.assertEqual((destination/'bin/limactl').read_bytes(),b'native-executable')
            self.assertTrue((destination/'share/lima/lima-guestagent.Linux-aarch64.gz').is_file())
            self.assertTrue((destination/'share/doc/lima/LICENSE').is_file())

    def test_corrupt_incomplete_or_escaping_archives_never_become_bundled_runtimes(self):
        runtime=module()
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);bundle=root/'bad.tar.gz'
            digest=archive(bundle,{'bin/limactl':b'partial'})
            for expected in ['0'*64,digest]:
                destination=root/'incomplete'
                with self.assertRaises(ValueError):runtime.extract_bundle(bundle,destination,expected,'aarch64')
                self.assertFalse(destination.exists())
            digest=archive(bundle,{'../escaped':b'bad'})
            with self.assertRaises((ValueError,tarfile.FilterError)):
                runtime.extract_bundle(bundle,root/'escaping',digest,'aarch64')
            self.assertFalse((root/'escaped').exists())

    def test_macos_builds_bundle_and_smoke_test_the_runtime(self):
        build=(ROOT/'scripts/build.py').read_text()
        smoke=(ROOT/'scripts/package_smoke.py').read_text()
        self.assertIn('bundle_lima(',build)
        self.assertIn('resources',build)
        self.assertIn('check_vm_runtime(',smoke)

