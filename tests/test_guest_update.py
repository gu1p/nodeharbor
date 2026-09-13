"""Guest updates preserve enrollment and atomically replace only packaged files."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
ID='9511182e-9c48-4d20-a15b-1da8bb441386'
FILES={
    '/usr/local/lib/nodeharbor/configure_worker.py':'0700',
    '/usr/local/lib/nodeharbor/watchdog.py':'0700',
    '/etc/systemd/system/nodeharbor-watchdog.service':'0644',
    '/etc/systemd/system/nodeharbor-watchdog.timer':'0644',
}

class GuestUpdate(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec=importlib.util.spec_from_file_location('install_guest',ROOT/'guest'/'install_guest.py')
        cls.installer=importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.installer)

    def setUp(self):
        self.directory=tempfile.TemporaryDirectory();self.addCleanup(self.directory.cleanup)
        self.root=Path(self.directory.name)
        marker=self.root/'etc/nodeharbor/device-id';marker.parent.mkdir(parents=True);marker.write_text(ID)
        (marker.parent/'lease').write_text('existing owner lease')
        self.payload={'deviceId':ID,'files':[{'path':path,'permissions':mode,'owner':'root:root','content':'new code\n'} for path,mode in FILES.items()]}
        for path in FILES:
            target=self.root/path.lstrip('/');target.parent.mkdir(parents=True,exist_ok=True);target.write_text('old code\n')

    def test_updates_apply_the_complete_bundle_and_preserve_device_credentials_and_lease(self):
        self.installer.install(self.payload,self.root)
        for path,mode in FILES.items():
            target=self.root/path.lstrip('/')
            self.assertEqual(target.read_text(),'new code\n')
            if os.name!='nt':self.assertEqual(target.stat().st_mode&0o777,int(mode,8))
        self.assertEqual((self.root/'etc/nodeharbor/device-id').read_text(),ID)
        self.assertEqual((self.root/'etc/nodeharbor/lease').read_text(),'existing owner lease')

    def test_wrong_owner_unknown_paths_and_incomplete_bundles_change_nothing(self):
        cases=[{'deviceId':'another-device','files':self.payload['files']},
               {'deviceId':ID,'files':self.payload['files'][:-1]},
               {'deviceId':ID,'files':self.payload['files']+[{'path':'/etc/nodeharbor/device-id','owner':'root:root','permissions':'0600','content':'replacement'}]},
               {'deviceId':ID,'files':self.payload['files']+[self.payload['files'][0]]}]
        for payload in cases:
            with self.subTest(payload=payload),self.assertRaises(ValueError):self.installer.install(payload,self.root)
            for path in FILES:self.assertEqual((self.root/path.lstrip('/')).read_text(),'old code\n')

    def test_failed_replacement_preserves_old_code_and_retry_converges_without_staging_residue(self):
        with patch.object(self.installer.os,'replace',side_effect=OSError('disk error')):
            with self.assertRaises(OSError):self.installer.install(self.payload,self.root)
        for path in FILES:self.assertEqual((self.root/path.lstrip('/')).read_text(),'old code\n')
        self.assertFalse(list(self.root.rglob('.nodeharbor-update-*')))
        self.installer.install(self.payload,self.root)
        for path in FILES:self.assertEqual((self.root/path.lstrip('/')).read_text(),'new code\n')

    @unittest.skipIf(os.name=='nt','The Linux guest uses POSIX file replacement')
    def test_running_reader_keeps_complete_old_code_during_replacement(self):
        path=self.root/'usr/local/lib/nodeharbor/watchdog.py'
        with path.open() as running:
            self.installer.install(self.payload,self.root)
            self.assertEqual(running.read(),'old code\n')
        self.assertEqual(path.read_text(),'new code\n')

    @unittest.skipIf(os.name=='nt','The Linux guest owns POSIX symbolic links')
    def test_symlinks_cannot_redirect_updates_outside_the_owned_files(self):
        target=self.root/'usr/local/lib/nodeharbor/watchdog.py';target.unlink()
        credential=self.root/'etc/nodeharbor/device-id';target.symlink_to(credential)
        with self.assertRaises(ValueError):self.installer.install(self.payload,self.root)
        self.assertEqual(credential.read_text(),ID)
