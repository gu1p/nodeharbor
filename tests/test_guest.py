import importlib.util
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch
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

class GuestNetworkConfiguration(unittest.TestCase):
    def prepare(self, resolver_text, install=None, fail_command=None):
        config=GuestContract().config()
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'device-id').write_text(config['deviceId'])
            resolver=root/'resolv.conf';resolver.write_text(resolver_text)
            writes={};commands=[]
            def run(*args, **kwargs):
                commands.append(args)
                if args==fail_command:raise RuntimeError('Guest service restart failed')
                return json.dumps({'netbirdIp':'100.75.1.2/16'}) if 'status' in args else ''
            with patch.object(configure,'ROOT',root), patch.object(configure,'RESOLV_CONF',resolver,create=True), patch.object(configure.sys,'platform','linux'), patch.object(configure.os,'geteuid',return_value=0,create=True), patch.object(configure.sys,'stdin',io.StringIO(json.dumps(config))), patch.object(configure,'install_runtime',new=install if install is not None else Mock()), patch.object(configure,'run',side_effect=run), patch.object(configure,'write',side_effect=lambda path,content,mode=0o600:writes.update({str(path):content})):
                configure.main()
            self.assertEqual(resolver.read_text(),resolver_text)
            return commands,writes,str(resolver)

    def test_slow_k3s_startup_waits_for_real_readiness_under_the_owners_deadline(self):
        import configparser
        _,writes,_=self.prepare('nameserver 192.168.64.1\n')
        service=configparser.ConfigParser(interpolation=None)
        service.read_string(writes['/etc/systemd/system/k3s-agent.service'])
        self.assertEqual(service['Service']['Type'],'notify')
        self.assertEqual(service['Service'].get('TimeoutStartSec'),'0',
            'K3s must not be killed by systemd before the supervising owner\'s bounded preparation operation expires')

    def test_preparation_restarts_services_to_apply_new_binaries_and_worker_configuration(self):
        commands,writes,_=self.prepare('nameserver 192.168.64.1\n')
        for service in ['netbird','k3s-agent']:
            self.assertIn(('systemctl','restart',service),commands,
                'Enabling an already-running service does not load its new executable or configuration')
            self.assertLess(commands.index(('systemctl','enable',service)),commands.index(('systemctl','restart',service)))
        up=next(args for args in commands if args[:2]==('/usr/local/bin/netbird','up'))
        self.assertLess(commands.index(('systemctl','restart','netbird')),commands.index(up))
        self.assertLess(commands.index(('systemctl','daemon-reload')),commands.index(('systemctl','restart','k3s-agent')))
        self.assertIn('/etc/rancher/k3s/config.yaml',writes)

    def test_preparation_reports_network_and_kubernetes_steps_without_bootstrap_secrets(self):
        output=io.StringIO()
        with patch.object(configure.sys,'stdout',output):
            self.prepare('nameserver 192.168.64.1\n')
        messages=output.getvalue()
        for step in ['Checking Ubuntu DNS','Starting NetBird','Connecting to the private network','Waiting for a private network address','Starting the Kubernetes worker','Worker configured; awaiting qualification']:
            self.assertIn('NodeHarbor step: '+step,messages)
        self.assertNotIn(GuestContract().config()['k3sToken'],messages)

    def test_failed_service_restart_cannot_report_successful_worker_preparation(self):
        for service in ['netbird','k3s-agent']:
            with self.subTest(service=service),self.assertRaisesRegex(RuntimeError,'restart failed'):
                self.prepare('nameserver 192.168.64.1\n',fail_command=('systemctl','restart',service))

    def test_preparation_preserves_guest_dns_and_uses_its_upstream_resolver_for_pods(self):
        commands,writes,resolver=self.prepare('nameserver 192.168.64.1\nsearch local.example\n')
        up=next(args for args in commands if args[:2]==('/usr/local/bin/netbird','up'))
        for flag in ['--disable-dns','--disable-server-routes','--allow-server-ssh=false']:
            self.assertIn(flag,up)
        self.assertIn('--disable-client-routes=false',up,'Keep the authenticated private API route available')
        k3s=json.loads(writes['/etc/rancher/k3s/config.yaml'])
        self.assertEqual(k3s['resolv-conf'],resolver)
        self.assertNotIn('/etc/rancher/k3s/resolv.conf',writes)
        self.assertEqual(k3s['flannel-iface'],'wt0')

    def test_kubernetes_tunnels_cannot_become_netbirds_transport(self):
        commands,_,_=self.prepare('nameserver 192.168.64.1\n')
        up=next(args for args in commands if args[:2]==('/usr/local/bin/netbird','up'))
        self.assertIn('--extra-iface-blacklist',up,
            'NetBird otherwise selects Flannel addresses after K3s starts, recursively tunneling its own transport')
        excluded=up[up.index('--extra-iface-blacklist')+1].split(',')
        for interface in ['flannel.1','cni0','kube-ipvs0']:
            self.assertTrue(any(interface.startswith(prefix) for prefix in excluded))
        for interface in ['eth0','enp7s0']:
            self.assertFalse(any(interface.startswith(prefix) for prefix in excluded))

    def test_unusable_guest_dns_is_reported_without_installing_or_starting_a_worker(self):
        install=Mock()
        with self.assertRaisesRegex(ValueError,'resolver'):
            self.prepare('nameserver 127.0.0.53\n',install=install)
        install.assert_not_called()

    def test_guest_preparation_cannot_renew_the_owners_lease_for_itself(self):
        commands,_,_=self.prepare('nameserver 192.168.64.1\n')
        self.assertFalse(any('watchdog.py' in ' '.join(args) and 'renew' in args for args in commands),
            'Only the supervising owner may keep a contributed worker alive')
        self.assertIn(('systemctl','enable','--now','nodeharbor-watchdog.timer'),commands)

class ResolverValidation(unittest.TestCase):
    def test_only_nonlocal_upstream_nameservers_are_usable_from_pods(self):
        cases=[('',False),('search example.com\n',False),('nameserver 127.0.0.53\n',False),
            ('nameserver ::1\n',False),('nameserver 169.254.1.1\n',False),
            ('nameserver 224.0.0.1\n',False),('nameserver invalid\n',False),
            ('nameserver 0.0.0.0\n',False),('nameserver\n',False),
            ('# DHCP resolvers\nnameserver 192.168.64.1\n',True),
            ('nameserver 10.0.0.53 # upstream\n',True),
            ('nameserver 192.168.64.1\nnameserver ::1\n',False)]
        for contents,valid in cases:
            with self.subTest(contents=contents), tempfile.TemporaryDirectory() as directory:
                resolver=Path(directory)/'resolv.conf';resolver.write_text(contents)
                if valid:configure.validate_resolver(resolver)
                else:
                    with self.assertRaisesRegex(ValueError,'resolver'):configure.validate_resolver(resolver)
