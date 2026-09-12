import importlib.util
from pathlib import Path
import unittest
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
