"""The macOS installer must include the complete verified VM runtime."""
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest
from unittest.mock import patch

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
    def test_linux_bundling_preserves_verified_runtime_bytes_without_changing_host_environment(self):
        runtime=module()
        host={'PATH':'owner-path', 'NO_STRIP':'owner-value'}
        for target in ['aarch64-unknown-linux-gnu','x86_64-unknown-linux-gnu']:
            env=runtime.bundle_environment(target,host)
            self.assertEqual(env['NO_STRIP'],'1')
            self.assertEqual(env['PATH'],'owner-path')
        for target in ['aarch64-apple-darwin','x86_64-pc-windows-msvc']:
            self.assertEqual(runtime.bundle_environment(target,host),host)
        self.assertEqual(host,{'PATH':'owner-path','NO_STRIP':'owner-value'})

    def test_linux_runtime_archives_are_pinned_for_both_native_architectures(self):
        runtime=module()
        expected={
            'aarch64-unknown-linux-gnu':('aarch64','7c6a09c6844f55f811e9b7b2b60a6070a512c696c8ec752dfdb3c8ed50ed0364'),
            'x86_64-unknown-linux-gnu':('x86_64','a0ea1ccf6b7335a900adb5f8d2b8384457965fecb1ba72f09b4e3e46d12f424a'),
        }
        for target,(arch,digest) in expected.items():
            with self.subTest(target=target):
                self.assertEqual(runtime.MANIFEST['archives'].get(target),{
                    'name':f'lima-2.2.0-Linux-{arch}.tar.gz','sha256':digest,'guestArch':arch})

    def test_linux_packages_include_lima_and_architecture_specific_qemu_dependencies(self):
        runtime=module()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve();(root/'bin').mkdir();(root/'bin/limactl').write_bytes(b'limactl')
            for target,emulator in [('aarch64-unknown-linux-gnu','qemu-system-arm'),('x86_64-unknown-linux-gnu','qemu-system-x86')]:
                with self.subTest(target=target),patch.object(runtime,'bundle_lima',return_value=root) as bundled:
                    config=runtime.bundle_configuration(target)
                    bundled.assert_called_once_with(target)
                    self.assertNotIn('resources',config)
                    for package in ['deb','appimage']:
                        self.assertEqual(config['linux'][package]['files'],
                                         {'/usr/libexec/nodeharbor/lima/bin/limactl':str(root/'bin/limactl')})
                    dependencies=config['linux']['deb']['depends']
                    for dependency in [emulator,'qemu-utils','openssh-client','gzip','libwebkit2gtk-4.1-0','libayatana-appindicator3-1','libxss1']:
                        self.assertIn(dependency,dependencies)
                    self.assertNotIn('multipass',dependencies)

    @unittest.skipIf(os.name=='nt','Linux bundle aliases require Unix symlinks')
    def test_linux_runtime_file_mapping_rejects_escaping_or_recursive_aliases(self):
        runtime=module()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve();source=root/'lima';source.mkdir()
            outside=root/'outside';outside.write_bytes(b'host file')
            alias=source/'alias';alias.symlink_to(outside)
            with self.assertRaisesRegex(ValueError,'inside the bundle without cycles'):
                runtime.bundle_files(source)
            alias.unlink();alias.symlink_to(source,target_is_directory=True)
            with self.assertRaisesRegex(ValueError,'inside the bundle without cycles'):
                runtime.bundle_files(source)

    @unittest.skipIf(os.name=='nt','Linux bundle aliases require Unix symlinks')
    def test_linux_package_mapping_preserves_verified_tools_and_directory_alias_contents(self):
        runtime=module()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve();source=root/'lima';bundle=root/'lima.tar.gz'
            digest=archive(bundle,{'bin/limactl':b'executable','share/lima/lima-guestagent.Linux-aarch64.gz':b'guest',
                                  'share/doc/lima/LICENSE':b'license','share/doc/lima/templates/default.yaml':b'template'})
            runtime.extract_bundle(bundle,source,digest,'aarch64')
            (source/'share/lima/templates').symlink_to('../doc/lima/templates',target_is_directory=True)
            (source/'bin/lima').symlink_to('limactl')
            with patch.object(runtime,'bundle_lima',return_value=source), patch.dict(runtime.MANIFEST['archives'],{'fixture':{'sha256':digest}}):
                config=runtime.bundle_configuration('aarch64-unknown-linux-gnu')
                for kind in ['deb','appimage']:
                    package=root/kind
                    for destination,original in config['linux'][kind]['files'].items():
                        self.assertTrue(Path(original).is_file(),'Package mappings must contain files, not directories')
                        path=package/destination.lstrip('/');path.parent.mkdir(parents=True,exist_ok=True)
                        shutil.copy2(original,path)
                    installed=package/'usr/libexec/nodeharbor/lima'
                    runtime.verify_bundle(installed)
                    self.assertEqual((installed/'share/lima/templates/default.yaml').read_bytes(),b'template')
                    self.assertEqual((installed/'bin/lima').read_bytes(),b'executable')
                    self.assertFalse(any(path.is_symlink() for path in installed.rglob('*')))

    def test_windows_packaging_keeps_its_existing_runtime_provider(self):
        runtime=module()
        with patch.object(runtime,'bundle_lima') as bundled:
            config=runtime.bundle_configuration('x86_64-pc-windows-msvc')
        bundled.assert_not_called()
        self.assertNotIn('resources',config)
        self.assertNotIn('linux',config)

    def test_linux_runtime_smoke_check_uses_the_shared_desktop_and_cli_resource_directory(self):
        runtime=module()
        for package in [Path('/package/deb'),Path('/package/squashfs-root')]:
            with self.subTest(package=package),patch.object(runtime,'verify_bundle') as verify,patch.object(runtime.subprocess,'check_output',return_value='limactl version 2.2.0\n') as command:
                runtime.check_vm_runtime(package,platform='linux')
                directory=package/'usr/libexec/nodeharbor/lima'
                verify.assert_called_once_with(directory)
                self.assertEqual(command.call_args.args[0],[str(directory/'bin/limactl'),'--version'])

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
        self.assertIn('bundle_configuration(args.target)',build)
        self.assertIn('check_vm_runtime(',smoke)
        runtime=module()
        with patch.object(runtime,'bundle_lima',return_value=Path('/verified/lima')) as bundled:
            config=runtime.bundle_configuration('aarch64-apple-darwin')
        bundled.assert_called_once_with('aarch64-apple-darwin')
        self.assertEqual(config['resources'],{str(Path('/verified/lima')):'lima/'})
        self.assertNotIn('linux',config)
