"""Android storage commands retain verified private backups and never accept host paths."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'guest' / (name + '.py'))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


pool, backup = module('storage_pool'), module('storage_backup')
with mock.patch.dict(sys.modules, {'storage_pool': pool, 'storage_backup': backup}):
    storage = module('android_storage')

OWNER = '9511182e-9c48-4d20-a15b-1da8bb441386'
OPERATION = '00000000-0000-4000-8000-000000000001'
POOL = '00000000-0000-4000-8000-000000000002'


class AndroidStorageContract(unittest.TestCase):
    def request(self, action='backup'):
        return dict(deviceId=OWNER, operation=OPERATION, action=action, previousPoolId=POOL)

    def test_fixed_storage_requests_reject_paths_other_owners_and_replacement_without_backup(self):
        storage.validate_request(self.request(), OWNER)
        for invalid in [dict(self.request(), path='/other'), dict(self.request(), deviceId=POOL),
                        dict(self.request(), action='exec'), dict(self.request(), operation='../backup')]:
            with self.assertRaises((ValueError, KeyError)): storage.validate_request(invalid, OWNER)
        request = dict(self.request('apply'), pool=dict(format=1, deviceId=OWNER, poolId=POOL, generation=1,
            disks=[dict(id='disk1', device='/dev/vdb', allocationBytes=15 * pool.GIB, initialize=True)]), restore=True)
        storage.validate_request(request, OWNER)
        request['pool']['disks'][0]['device'] = '/dev/vda'
        with self.assertRaises(ValueError): storage.validate_request(request, OWNER)

    def test_verified_private_backup_can_be_replayed_without_changing_the_original(self):
        if os.name == 'nt': self.skipTest('Backup files run inside the Linux guest')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); source = root / 'source'; source.mkdir()
            (source / 'owner-marker').write_bytes(b'previously flushed worker data\x00\xff')
            work = root / 'private'; work.mkdir()
            receipt = storage.create_backup(source, work, self.request(), minimum_free=0)
            self.assertTrue(receipt['verified'])
            self.assertEqual(receipt['operation'], OPERATION)
            self.assertEqual(receipt['poolId'], POOL)
            target = root / 'target'; target.mkdir()
            storage.restore_verified_backup(target, work, self.request())
            self.assertEqual((source / 'owner-marker').read_bytes(), (target / 'owner-marker').read_bytes())
            storage.restore_verified_backup(target, work, self.request())
            self.assertEqual((target / 'owner-marker').read_bytes(), b'previously flushed worker data\x00\xff')
            work.joinpath(OPERATION + '.backup').write_bytes(b'corrupt')
            with self.assertRaises(ValueError): storage.restore_verified_backup(target, work, self.request())
            self.assertEqual((target / 'owner-marker').read_bytes(), b'previously flushed worker data\x00\xff')

    def test_insufficient_private_backup_capacity_preserves_the_source(self):
        if os.name == 'nt': self.skipTest('Backup files run inside the Linux guest')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); source = root / 'source'; source.mkdir()
            (source / 'marker').write_text('retained')
            work = root / 'private'; work.mkdir()
            with self.assertRaises(ValueError): storage.create_backup(source, work, self.request(), minimum_free=2**63)
            self.assertEqual((source / 'marker').read_text(), 'retained')
            self.assertEqual(list(work.iterdir()), [])

    def test_a_crash_between_backup_receipt_and_rename_can_resume_the_same_operation(self):
        if os.name == 'nt': self.skipTest('Backup files run inside the Linux guest')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); source = root / 'source'; source.mkdir()
            (source / 'marker').write_text('preserved')
            work = root / 'private'; work.mkdir()
            with mock.patch.object(Path, 'replace', side_effect=OSError('interrupted final rename')):
                with self.assertRaises(OSError): storage.create_backup(source, work, self.request(), minimum_free=0)
            self.assertTrue(storage.create_backup(source, work, self.request(), minimum_free=0)['verified'])
            self.assertEqual((source / 'marker').read_text(), 'preserved')

    def test_backup_cleanup_can_resume_after_the_archive_was_already_removed(self):
        if os.name == 'nt': self.skipTest('Backup files run inside the Linux guest')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); source = root / 'source'; source.mkdir()
            (source / 'marker').write_text('preserved')
            work = root / 'private'; work.mkdir()
            storage.create_backup(source, work, self.request(), minimum_free=0)
            work.joinpath(OPERATION + '.backup').unlink()
            config = root / 'config'; config.mkdir(); (config / 'device-id').write_text(OWNER)
            with mock.patch.object(storage, 'CONFIG', config), mock.patch.object(storage, 'PRIVATE', work), \
                    mock.patch.object(pool, 'check_pool', return_value={'poolId': '00000000-0000-4000-8000-000000000004'}):
                self.assertTrue(storage.operate(self.request('cleanup'))['cleaned'])
                self.assertTrue(storage.operate(self.request('cleanup'))['cleaned'])
