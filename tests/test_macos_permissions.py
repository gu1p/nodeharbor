"""macOS pilot packages must identify the app requesting external-disk access."""
import json
import importlib.util
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def smoke_module():
    spec = importlib.util.spec_from_file_location('package_smoke', ROOT / 'scripts/package_smoke.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def app_fixture(root):
    app = root / 'NodeHarbor.app'
    (app / 'Contents/MacOS').mkdir(parents=True)
    (app / 'Contents/Resources/lima/bin').mkdir(parents=True)
    for name in ['nodeharbor', 'nodeharbor-agent']:
        (app / 'Contents/MacOS' / name).touch()
    (app / 'Contents/Resources/lima/bin/limactl').touch()
    info = {'CFBundleIdentifier': 'io.github.gu1p.nodeharbor', 'CFBundleExecutable': 'nodeharbor',
            'CFBundleShortVersionString': '0.1.0', 'CFBundleVersion': '0.1.0',
            'NSRemovableVolumesUsageDescription': 'NodeHarbor uses your external temporary folder and worker storage.'}
    (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
    return app, info


class MacosPermissionBehavior(unittest.TestCase):
    def test_pilot_bundle_is_signed_and_explains_why_it_needs_removable_volumes(self):
        config = json.loads((ROOT / 'desktop/tauri.conf.json').read_text())
        macos = config['bundle']['macOS']
        self.assertEqual(macos.get('signingIdentity'), '-')
        self.assertEqual(config['identifier'], 'io.github.gu1p.nodeharbor')
        with (ROOT / 'desktop' / macos['infoPlist']).open('rb') as source:
            info = plistlib.load(source)
        self.assertEqual(info['NSRemovableVolumesUsageDescription'],
                         'NodeHarbor needs access to external drives to use your configured temporary folder and worker storage.')


class MacosBundleValidation(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.app, self.info = app_fixture(Path(self.temporary.name))
        self.smoke = smoke_module()
        self.entitlements = {f'com.apple.security.{name}': True
                             for name in ['virtualization', 'network.client', 'network.server']}

    def display(self, args, **kwargs):
        if '--entitlements' in args:
            return plistlib.dumps(self.entitlements)
        return b'Identifier=io.github.gu1p.nodeharbor\nSignature=adhoc\n'

    def test_valid_bundle_is_verified_before_any_packaged_program_runs(self):
        events = []
        with patch.object(self.smoke, 'run', side_effect=lambda args, **kw: events.append(('verify', args))), \
             patch.object(self.smoke.subprocess, 'check_output', side_effect=self.display), \
             patch.object(self.smoke, 'check_executable', side_effect=lambda *args: events.append(('execute', args))), \
             patch.object(self.smoke, 'check_vm_runtime') as runtime:
            self.smoke.check_macos_bundle(self.app, '0.1.0', 'a' * 40)
        self.assertEqual(events[0][0], 'verify')
        self.assertIn('--strict', events[0][1])
        self.assertIn('--deep', events[0][1])
        self.assertEqual(events[0][1][-1], str(self.app))
        executions = [args for kind, args in events if kind == 'execute']
        self.assertEqual(executions, [(self.app / 'Contents/MacOS' / name, '0.1.0', 'a' * 40)
                                      for name in ['nodeharbor', 'nodeharbor-agent']])
        runtime.assert_called_once_with(self.app)

    def test_invalid_signature_prevents_execution(self):
        with patch.object(self.smoke, 'run', side_effect=subprocess.CalledProcessError(1, 'codesign')), \
             patch.object(self.smoke, 'check_executable') as execute, \
             patch.object(self.smoke, 'check_vm_runtime') as runtime:
            with self.assertRaises(subprocess.CalledProcessError):
                self.smoke.check_macos_bundle(self.app, '0.1.0', 'a' * 40)
        execute.assert_not_called()
        runtime.assert_not_called()

    def test_missing_identity_permission_description_or_matching_version_is_rejected(self):
        for key, value in [('CFBundleIdentifier', 'another.app'), ('CFBundleExecutable', 'other'),
                           ('CFBundleShortVersionString', '0.0.9'), ('CFBundleVersion', '0.0.9'),
                           ('NSRemovableVolumesUsageDescription', ''), ('NSRemovableVolumesUsageDescription', None)]:
            with self.subTest(key=key, value=value):
                (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({
                    **{k: v for k, v in self.info.items() if k != key}, **({key: value} if value is not None else {})}))
                with patch.object(self.smoke, 'run'), \
                     patch.object(self.smoke.subprocess, 'check_output', side_effect=self.display), \
                     patch.object(self.smoke, 'check_executable') as execute:
                    with self.assertRaises(ValueError):
                        self.smoke.check_macos_bundle(self.app, '0.1.0', 'a' * 40)
                execute.assert_not_called()

    def test_signing_identity_and_lima_entitlements_are_required(self):
        for key in self.entitlements:
            with self.subTest(entitlement=key):
                self.entitlements[key] = False
                with patch.object(self.smoke, 'run'), \
                     patch.object(self.smoke.subprocess, 'check_output', side_effect=self.display), \
                     patch.object(self.smoke, 'check_executable') as execute:
                    with self.assertRaises(ValueError):
                        self.smoke.check_macos_bundle(self.app, '0.1.0', 'a' * 40)
                execute.assert_not_called()
                self.entitlements[key] = True
        with patch.object(self.smoke, 'run'), \
             patch.object(self.smoke.subprocess, 'check_output', return_value=b'Identifier=wrong\n'), \
             patch.object(self.smoke, 'check_executable') as execute:
            with self.assertRaises(ValueError):
                self.smoke.check_macos_bundle(self.app, '0.1.0', 'a' * 40)
        execute.assert_not_called()

    def test_missing_sidecar_is_rejected(self):
        (self.app / 'Contents/MacOS/nodeharbor-agent').unlink()
        with patch.object(self.smoke, 'run'), \
             patch.object(self.smoke.subprocess, 'check_output', side_effect=self.display), \
             patch.object(self.smoke, 'check_executable') as execute:
            with self.assertRaises(ValueError):
                self.smoke.check_macos_bundle(self.app, '0.1.0', 'a' * 40)
        execute.assert_not_called()


class MacosPackageIntegration(unittest.TestCase):
    def test_both_update_archive_and_installer_use_full_bundle_validation(self):
        smoke = smoke_module()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app, _ = app_fixture(root / 'source')
            target = 'aarch64-apple-darwin'
            with tarfile.open(root / f'nodeharbor-v0.1.0-{target}.app.tar.gz', 'w:gz') as archive:
                archive.add(app, arcname=app.name)
            with patch.object(smoke, 'mounted_image') as mount, \
                 patch.object(smoke, 'check_macos_bundle', create=True) as validate, \
                 patch.object(smoke, 'check_executable'), patch.object(smoke, 'check_vm_runtime'):
                mount.return_value.__enter__.return_value = app.parent
                smoke.smoke_packages(root, target, '0.1.0', 'a' * 40)
            self.assertEqual(validate.call_count, 2)
            self.assertEqual(validate.call_args_list[1].args, (app, '0.1.0', 'a' * 40))
            self.assertEqual(validate.call_args_list[0].args[0].name, 'NodeHarbor.app')

    @unittest.skipUnless(sys.platform == 'darwin', 'Requires native Apple codesign and compiler')
    def test_native_codesign_rejects_unsealed_and_tampered_bundles_and_accepts_sealed_copy(self):
        smoke = smoke_module()
        with tempfile.TemporaryDirectory(prefix='nodeharbor-signature-test-') as temporary:
            root = Path(temporary)
            app, _ = app_fixture(root)
            binary = app / 'Contents/MacOS/nodeharbor'
            source = '#include <stdio.h>\nint main(void) { puts("NodeHarbor 0.1.0 (' + 'a' * 40 + ')"); return 0; }\n'
            subprocess.run(['/usr/bin/cc', '-x', 'c', '-', '-o', str(binary)], input=source, text=True, check=True)
            sidecar = app / 'Contents/MacOS/nodeharbor-agent'
            shutil.copyfile(binary, sidecar)
            sidecar.chmod(0o755)
            lima = app / 'Contents/Resources/lima/bin/limactl'
            shutil.copyfile(binary, lima)
            lima.chmod(0o755)
            entitlements = root / 'entitlements.plist'
            entitlements.write_bytes(plistlib.dumps({f'com.apple.security.{key}': True
                                                     for key in ['virtualization', 'network.client', 'network.server']}))
            with patch.object(smoke, 'check_vm_runtime'):
                with self.assertRaises(subprocess.CalledProcessError):
                    smoke.check_macos_bundle(app, '0.1.0', 'a' * 40)
                for path, args in [(lima, ['--entitlements', str(entitlements)]), (sidecar, []), (app, [])]:
                    subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', *args, str(path)], check=True,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                smoke.check_macos_bundle(app, '0.1.0', 'a' * 40)
                installed = root / 'Installed/NodeHarbor.app'
                shutil.copytree(app, installed)
                smoke.check_macos_bundle(installed, '0.1.0', 'a' * 40)
                (installed / 'Contents/Resources/tampered.txt').write_text('unsealed data')
                with self.assertRaises(subprocess.CalledProcessError):
                    smoke.check_macos_bundle(installed, '0.1.0', 'a' * 40)
