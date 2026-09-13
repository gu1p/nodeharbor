import importlib.util
import hashlib
import io
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1]
def module(name):
    spec=importlib.util.spec_from_file_location(name,ROOT/'guest'/f'{name}.py')
    result=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result
configure=module('configure_worker')
watchdog=module('watchdog')

class GuestContract(unittest.TestCase):
    def config(self):
        return {'deviceId':'9511182e-9c48-4d20-a15b-1da8bb441386','nodeName':'nodeharbor-9511182e9c484d20a15b1da8bb441386','netbirdManagementUrl':'https://netbird.example.com','netbirdSetupKey':'example-one-off-key','serverUrl':'https://10.50.0.2:6443','k3sToken':'K10'+'a'*64+'::abcdef.'+'b'*16,'runtime':{'k3sVersion':'v1.36.4+k3s1','netbirdVersion':'0.78.1','assets':[]}}
    def test_join_credentials_require_a_pinned_cluster_identity(self):
        config=self.config()
        configure.validate_config(config)
        config['k3sToken']='insecure-short-token'
        with self.assertRaises(ValueError):configure.validate_config(config)
    def test_plain_http_and_cross_device_configuration_are_rejected(self):
        config=self.config();config['netbirdManagementUrl']='http://example.com'
        with self.assertRaises(ValueError):configure.validate_config(config)
        config=self.config();config['nodeName']='unrelated-node'
        with self.assertRaises(ValueError):configure.validate_config(config)
    def test_integrity_failure_prevents_using_a_download(self):
        with self.assertRaises(ValueError):configure.verify_download(b'changed archive','0'*64)
    def test_expired_or_missing_leases_stop_a_contributed_worker(self):
        self.assertFalse(watchdog.lease_expired(100.0,150.0,120))
        self.assertTrue(watchdog.lease_expired(100.0,220.0,120))
        self.assertTrue(watchdog.lease_expired(None,150.0,120))
        self.assertTrue(watchdog.lease_expired(200.0,150.0,120))

    def test_boot_grace_is_bounded_and_does_not_accept_a_future_lease(self):
        self.assertFalse(watchdog.lease_expired(None,15.0,120))
        self.assertTrue(watchdog.lease_expired(None,120.0,120))
        self.assertTrue(watchdog.lease_expired(200.0,15.0,120))

class RuntimeInstallation(unittest.TestCase):
    def runtime(self):
        archive=io.BytesIO()
        with tarfile.open(fileobj=archive,mode='w:gz') as bundle:
            member=tarfile.TarInfo('netbird');member.size=len(b'new netbird')
            bundle.addfile(member,io.BytesIO(b'new netbird'))
        contents={'netbird_0.78.1_linux_amd64.tar.gz':archive.getvalue(),'k3s':b'new k3s'}
        config={'runtime':{'netbirdVersion':'0.78.1','k3sVersion':'v1.36.4+k3s1',
            'assets':[{'name':name,'sha256':hashlib.sha256(value).hexdigest()} for name,value in contents.items()]}}
        return config,contents

    @unittest.skipIf(os.name=='nt','The Linux guest uses POSIX executable replacement semantics')
    def test_preparation_replaces_binaries_without_overwriting_running_executables(self):
        config,contents=self.runtime()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);binary=root/'bin'/'netbird';binary.parent.mkdir();binary.write_bytes(b'running netbird')
            with binary.open('rb') as running, patch.object(configure.platform,'machine',return_value='x86_64'), patch.object(configure,'download',side_effect=lambda url,checksum:contents[url.rsplit('/',1)[1]]):
                configure.install_runtime(config,bin_dir=binary.parent,cache_dir=root/'cache')
                self.assertEqual(running.read(),b'running netbird')
                self.assertEqual(binary.read_bytes(),b'new netbird')
                self.assertEqual((binary.parent/'k3s').read_bytes(),b'new k3s')

    def test_failed_atomic_replacement_preserves_old_file_and_removes_staging_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'runtime';path.write_bytes(b'old executable')
            with patch.object(configure.os,'replace',side_effect=OSError('disk error')):
                with self.assertRaises(OSError):configure.write_bytes(path,b'new executable',0o755)
            self.assertEqual(path.read_bytes(),b'old executable')
            self.assertEqual(list(path.parent.iterdir()),[path])

    def test_retry_uses_only_checksum_verified_cached_downloads(self):
        config,contents=self.runtime()
        with tempfile.TemporaryDirectory() as directory, patch.object(configure.platform,'machine',return_value='x86_64'):
            root=Path(directory)
            with patch.object(configure,'download',side_effect=lambda url,checksum:contents[url.rsplit('/',1)[1]]) as download:
                configure.install_runtime(config,bin_dir=root/'bin',cache_dir=root/'cache')
                self.assertEqual(download.call_count,2)
            with patch.object(configure,'download',side_effect=AssertionError('Verified cache should support retry offline')):
                configure.install_runtime(config,bin_dir=root/'bin',cache_dir=root/'cache')
            cached=root/'cache'/config['runtime']['assets'][0]['sha256'];cached.write_bytes(b'corrupted')
            with patch.object(configure,'download',side_effect=lambda url,checksum:contents[url.rsplit('/',1)[1]]) as download:
                configure.install_runtime(config,bin_dir=root/'bin',cache_dir=root/'cache')
                self.assertEqual(download.call_count,1)

    def test_second_download_failure_leaves_both_installed_runtimes_unchanged(self):
        config,contents=self.runtime()
        with tempfile.TemporaryDirectory() as directory, patch.object(configure.platform,'machine',return_value='x86_64'):
            root=Path(directory);binary=root/'bin';binary.mkdir()
            for name in ['netbird','k3s']:(binary/name).write_bytes(b'old '+name.encode())
            with patch.object(configure,'download',side_effect=[next(iter(contents.values())),OSError('offline')]):
                with self.assertRaises(OSError):configure.install_runtime(config,bin_dir=binary,cache_dir=root/'cache')
            for name in ['netbird','k3s']:self.assertEqual((binary/name).read_bytes(),b'old '+name.encode())
