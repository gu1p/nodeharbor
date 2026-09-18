"""Owned guest storage must never adopt, replace, or silently lose another disk."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path, PureWindowsPath
import shutil
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('storage_pool', ROOT / 'guest/storage_pool.py')
storage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(storage)

OWNER = '9511182e-9c48-4d20-a15b-1da8bb441386'
POOL = 'cf674529-d5e1-47f5-a2ca-b94695b39006'
PARTITION = 'befb532e-48e4-4968-880e-4c00bf3c5700'
FILESYSTEM = 'a60118f2-7765-4251-a0d6-4c13c950586a'
PV = 'aaaaaa-bbbb-cccc-dddd-eeee-ffff-gggggg'
GIB = 1024 ** 3


def request():
    return {'format': 1, 'deviceId': OWNER, 'poolId': POOL, 'generation': 1,
            'disks': [{'id': 'nh123456789', 'device': '/dev/vdb',
                       'allocationBytes': 16 * GIB, 'initialize': True}]}


def previous():
    value = request()
    value['disks'][0].update({'initialize': False, 'partitionUuid': PARTITION, 'pvUuid': PV})
    value.update({'filesystemUuid': FILESYSTEM, 'vgUuid': 'owned-vg', 'lvUuid': 'owned-lv'})
    return value


def empty_disk(path='/dev/vdb'):
    return {'path': path, 'type': 'disk', 'sizeBytes': 16 * GIB,
            'containsRoot': False, 'mountpoints': [], 'signatures': [], 'partitions': []}


def owned_disk(path='/dev/vdb'):
    value = empty_disk(path)
    value['signatures'] = ['gpt']
    value['partitions'] = [{'path': path + '1', 'partitionUuid': PARTITION,
                            'pvUuid': PV, 'filesystem': 'LVM2_member', 'mountpoints': []}]
    return value


class GuestStorageRequest(unittest.TestCase):
    def test_guest_paths_resolve_inside_the_owned_root_with_windows_path_objects(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.assertEqual(storage.path_at(root, PureWindowsPath('/etc/nodeharbor/device-id')),
                             root / 'etc/nodeharbor/device-id')

    def test_a_request_is_bound_to_the_enrolled_guest_and_pool(self):
        storage.validate_request(request(), OWNER)
        with self.assertRaisesRegex(ValueError, 'owner|device'):
            storage.validate_request(request(), 'ba3d28c8-74c8-4340-9f71-039e5676003c')
        for field, value in [('deviceId', 'not-a-device'), ('poolId', 'not-a-pool')]:
            candidate = request(); candidate[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)

    def test_only_bounded_additional_virtio_disks_can_be_selected(self):
        for device in ['/dev/vda', '/dev/sda', '/dev/vdb1', '/dev/../dev/vdb', '/tmp/disk', '/dev/vdb;reboot']:
            candidate = request(); candidate['disks'][0]['device'] = device
            with self.subTest(device=device), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)

    def test_allocation_and_generation_require_bounded_integers(self):
        for allocation in [True, 0, -GIB, GIB + 1, 1.5 * GIB, 2 ** 64]:
            candidate = request(); candidate['disks'][0]['allocationBytes'] = allocation
            with self.subTest(allocation=allocation), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)
        for generation in [True, -1, 1.5, 2 ** 64]:
            candidate = request(); candidate['generation'] = generation
            with self.subTest(generation=generation), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)

    def test_internal_names_cannot_trigger_limas_truncated_label_bug(self):
        for name in ['', 'twelvechars1', '../other', 'disk space', 'dísk', 'a' * 12]:
            candidate = request(); candidate['disks'][0]['id'] = name
            with self.subTest(name=name), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)

    def test_duplicates_and_empty_or_unbounded_pools_are_rejected(self):
        for disks in [[], request()['disks'] * 2, request()['disks'] * 17]:
            candidate = request(); candidate['disks'] = disks
            with self.subTest(count=len(disks)), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)
        for changed in [{'id': 'another'}, {'device': '/dev/vdc'}]:
            candidate = request()
            candidate['disks'].append({**candidate['disks'][0], **changed})
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)

    def test_initialization_is_explicit_and_boolean(self):
        for value in [None, 1, 'true']:
            candidate = request(); candidate['disks'][0]['initialize'] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                storage.validate_request(candidate, OWNER)


class GuestStorageChanges(unittest.TestCase):
    def test_retry_and_growth_preserve_the_existing_pool_identity(self):
        candidate = request(); candidate['disks'][0]['initialize'] = False
        storage.validate_transition(candidate, previous())
        candidate['generation'] = 2
        candidate['disks'][0]['allocationBytes'] = 24 * GIB
        candidate['disks'].append({'id': 'nh987654321', 'device': '/dev/vdc',
                                   'allocationBytes': 12 * GIB, 'initialize': True})
        storage.validate_transition(candidate, previous())

    def test_existing_members_cannot_be_removed_reinitialized_or_shrunk(self):
        candidates = []
        candidate = request(); candidate['disks'] = []; candidates.append(candidate)
        candidate = request(); candidates.append(candidate)
        candidate = request(); candidate['disks'][0].update(initialize=False, allocationBytes=8 * GIB); candidates.append(candidate)
        for candidate in candidates:
            candidate['generation'] = 2
            with self.subTest(candidate=candidate), self.assertRaises(ValueError):
                storage.validate_transition(candidate, previous())

    def test_stale_or_reused_generations_cannot_change_storage(self):
        for generation in [0, 1]:
            candidate = request(); candidate['generation'] = generation
            candidate['disks'][0].update(initialize=False, allocationBytes=24 * GIB)
            with self.subTest(generation=generation), self.assertRaises(ValueError):
                storage.validate_transition(candidate, previous())

    def test_another_pool_or_owner_cannot_adopt_the_recorded_disks(self):
        for field in ['poolId', 'deviceId']:
            candidate = request(); candidate['disks'][0]['initialize'] = False
            candidate[field] = 'ba3d28c8-74c8-4340-9f71-039e5676003c'
            with self.subTest(field=field), self.assertRaises(ValueError):
                storage.validate_transition(candidate, previous())


class GuestStorageDevices(unittest.TestCase):
    def test_a_fresh_owned_request_can_initialize_only_a_blank_additional_disk(self):
        result = storage.plan_devices(request(), None, [empty_disk()])
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]['device'], '/dev/vdb')
        self.assertTrue(result[0]['initialize'])

    def test_existing_filesystems_partitions_mounts_and_root_are_never_initialized(self):
        cases = [{'signatures': ['ext4']}, {'signatures': ['gpt']},
                 {'partitions': [{'path': '/dev/vdb1'}]}, {'mountpoints': ['/data']},
                 {'containsRoot': True}, {'type': 'loop'}]
        for changed in cases:
            disk = empty_disk(); disk.update(changed)
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                storage.plan_devices(request(), None, [disk])

    def test_absent_or_undersized_devices_never_fall_back_to_another_disk(self):
        for disks in [[], [empty_disk('/dev/vdc')], [{**empty_disk(), 'sizeBytes': 15 * GIB}]]:
            with self.subTest(disks=disks), self.assertRaises(ValueError):
                storage.plan_devices(request(), None, disks)

    def test_a_fresh_disk_without_initialization_authorization_is_rejected(self):
        candidate = request(); candidate['disks'][0]['initialize'] = False
        with self.assertRaises(ValueError):
            storage.plan_devices(candidate, None, [empty_disk()])

    def test_existing_members_are_resolved_by_partition_and_pv_uuid_after_reordering(self):
        candidate = request(); candidate['disks'][0]['initialize'] = False
        result = storage.plan_devices(candidate, previous(), [owned_disk('/dev/vdc'), empty_disk()])
        self.assertEqual(result[0]['device'], '/dev/vdc')
        self.assertEqual(result[0]['partition'], '/dev/vdc1')
        self.assertFalse(result[0]['initialize'])

    def test_missing_changed_and_duplicate_member_identities_are_rejected(self):
        changed_pv = owned_disk(); changed_pv['partitions'][0]['pvUuid'] = 'foreign-pv'
        changed_partition = owned_disk(); changed_partition['partitions'][0]['partitionUuid'] = POOL
        candidate = request(); candidate['disks'][0]['initialize'] = False
        for disks in [[], [empty_disk()], [changed_pv], [changed_partition], [owned_disk(), owned_disk('/dev/vdc')]]:
            with self.subTest(disks=disks), self.assertRaises(ValueError):
                storage.plan_devices(candidate, previous(), disks)

    def test_preflight_does_not_mutate_requests_state_or_inventory(self):
        candidate = request(); state = None; inventory = [empty_disk()]
        before = copy.deepcopy((candidate, state, inventory))
        storage.plan_devices(candidate, state, inventory)
        self.assertEqual((candidate, state, inventory), before)


class GuestStorageMount(unittest.TestCase):
    def test_lvm_reports_each_segment_without_turning_one_linear_volume_into_multiple_volumes(self):
        state = previous(); vg_name, lv_name = storage.names(state)
        tags = ','.join(sorted(storage.tags(state)))
        volume = {'vg_name': vg_name, 'lv_name': lv_name, 'lv_uuid': state['lvUuid'], 'lv_tags': tags, 'segtype': 'linear'}
        def execute(*args, **kwargs):
            responses = {
                'vgs': ('vg', [{'vg_name': vg_name, 'vg_uuid': state['vgUuid'], 'vg_tags': tags, 'vg_free_count': '0'}]),
                'lvs': ('lv', [volume, dict(volume)]),
                'pvs': ('pv', [{'pv_name': '/dev/vdb1', 'pv_uuid': PV, 'vg_name': vg_name}]),
            }
            key, entries = responses[args[0]]
            return json.dumps({'report': [{key: entries}]})
        group, logical = storage.validate_lvm(state, execute)
        self.assertEqual(group['vg_uuid'], state['vgUuid'])
        self.assertEqual(logical['lv_uuid'], state['lvUuid'])
        for changed in [{'segtype': 'thin'}, {'lv_uuid': 'foreign-lv'}, {'lv_name': 'foreign'}]:
            def invalid(*args, **kwargs):
                if args[0] == 'lvs':return json.dumps({'report': [{'lv': [volume, {**volume, **changed}]}]})
                return execute(*args, **kwargs)
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                storage.validate_lvm(state, invalid)

    def test_readiness_requires_the_owned_ext4_filesystem_at_the_exact_pool_mount(self):
        mount = {'target': '/var/lib/nodeharbor/storage', 'fstype': 'ext4',
                 'uuid': FILESYSTEM, 'options': 'rw,relatime'}
        storage.validate_mount(previous(), mount)
        for changed in [{'target': '/'}, {'fstype': 'fuse.mergerfs'}, {'uuid': POOL}, {'options': 'ro,relatime'}]:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                storage.validate_mount(previous(), {**mount, **changed})
        with self.assertRaises(ValueError):
            storage.validate_mount(previous(), None)


@unittest.skipIf(os.name == 'nt', 'Guest command and filesystem lifecycle requires POSIX semantics')
class GuestStorageExecution(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(); self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.config = self.root / 'etc/nodeharbor'; self.config.mkdir(parents=True)
        (self.config / 'device-id').write_text(OWNER)
        self.commands = []

    def execute(self, *args, **kwargs):
        self.commands.append(args)
        if args[:2] == ('systemctl', 'show'):return 'inactive\n'
        if args[0] == 'findmnt':return json.dumps({'filesystems': []})
        return ''

    def save_state(self, state=None):
        (self.config / 'storage-state.json').write_text(json.dumps(state or previous()))

    def test_findmnt_no_match_is_an_unmounted_pool_not_invalid_json(self):
        self.assertIsNone(storage.pool_mount(self.root, lambda *args, **kwargs: ''))

    def test_activation_requires_every_pv_and_never_uses_partial_mode(self):
        self.save_state()
        mount = {'target': '/var/lib/nodeharbor/storage', 'fstype': 'ext4', 'uuid': FILESYSTEM, 'options': 'rw'}
        with patch.object(storage, 'inspect_devices', return_value=[owned_disk()]), \
             patch.object(storage, 'validate_lvm'), patch.object(storage, 'pool_mount', return_value=mount), \
             patch.object(storage, 'check_pool', return_value=previous()):
            storage.activate_pool(root=self.root, execute=self.execute)
        command = next(command for command in self.commands if command[0] == 'lvchange')
        self.assertEqual(command[command.index('--activationmode') + 1], 'complete')
        self.assertFalse(any(command[0] in {'sfdisk', 'pvcreate', 'mkfs.ext4'} for command in self.commands))

    def test_interruption_after_pv_creation_resumes_journaled_identity_without_reformatting(self):
        disk = empty_disk()
        interruption = [True]
        def execute(*args, **kwargs):
            self.commands.append(args)
            if args[:2] == ('systemctl', 'show'):return 'inactive\n'
            if args[0] == 'sfdisk':
                member = json.loads((self.config / 'storage-pending.json').read_text())['members'][0]
                disk['signatures'] = ['gpt']
                disk['partitions'] = [{'path': '/dev/vdb1', 'partitionUuid': member['partitionUuid'],
                                       'pvUuid': None, 'filesystem': None, 'mountpoints': []}]
            if args[0] == 'wipefs':return json.dumps({'signatures': []})
            if args[0] == 'pvcreate':
                disk['partitions'][0].update(pvUuid=args[args.index('--uuid') + 1], filesystem='LVM2_member')
                if interruption[0]:interruption[0] = False; raise RuntimeError('owner disconnected after pvcreate')
            if args[0] == 'pvs':
                entries = [{'pv_name': '/dev/vdb1', 'pv_uuid': disk['partitions'][0]['pvUuid'], 'vg_name': ''}]
                return json.dumps({'report': [{'pv': entries}]})
            if args[0] == 'vgs':return json.dumps({'report': [{'vg': []}]})
            if args[0] == 'vgcreate':raise RuntimeError('stop test before group creation')
            return ''
        with patch.object(storage, 'inspect_devices', side_effect=lambda execute: copy.deepcopy([disk])):
            with self.assertRaisesRegex(RuntimeError, 'disconnected'):
                storage.apply_request(request(), root=self.root, execute=execute)
            pending = (self.config / 'storage-pending.json').read_text()
            with self.assertRaisesRegex(RuntimeError, 'before group creation'):
                storage.apply_request(request(), root=self.root, execute=execute)
        self.assertEqual((self.config / 'storage-pending.json').read_text(), pending)
        for program in ('sfdisk', 'pvcreate'):
            self.assertEqual(sum(command[0] == program for command in self.commands), 1)
        self.assertFalse((self.config / 'storage-state.json').exists())

    def test_committed_retry_only_checks_activation_and_preserves_completed_migration(self):
        state = previous(); state.update(migrationComplete=True, appliedRequestDigest=storage.fingerprint(request()))
        self.save_state(state)
        pending = {'requestDigest': storage.fingerprint(request()), 'deviceId': OWNER, 'oldState': None}
        (self.config / 'storage-pending.json').write_text(json.dumps(pending))
        with patch.object(storage, 'activate_pool', return_value=state) as activate:
            result = storage.apply_request(request(), root=self.root, execute=self.execute)
        activate.assert_called_once()
        self.assertTrue(result['migrationComplete'])
        self.assertTrue(json.loads((self.config / 'storage-state.json').read_text())['migrationComplete'])
        self.assertFalse((self.config / 'storage-pending.json').exists())
        self.assertFalse(any(command[0] in {'sfdisk', 'pvcreate', 'pvresize', 'lvextend', 'resize2fs'} for command in self.commands))

    def test_committed_retry_restores_the_required_pool_service_before_activation(self):
        state = previous(); state.update(migrationComplete=True, appliedRequestDigest=storage.fingerprint(request()))
        self.save_state(state)
        unit = self.root / 'etc/systemd/system/nodeharbor-storage.service'
        def activate(*args, **kwargs):
            self.assertTrue(unit.is_file(), 'The committed pool still requires a boot activation service')
            self.assertIn('storage_pool.py activate', unit.read_text())
            self.assertIn(('systemctl', 'enable', 'nodeharbor-storage.service'), self.commands)
            return state
        with patch.object(storage, 'activate_pool', side_effect=activate):
            storage.apply_request(request(), root=self.root, execute=self.execute)
        self.assertEqual(unit.stat().st_mode & 0o777, 0o644)
        self.assertEqual(json.loads((self.config / 'storage-state.json').read_text()), state)

    def test_boot_resumes_the_owned_add_disk_journal_after_vgextend_before_state_commit(self):
        old = previous(); old['migrationComplete'] = True; self.save_state(old)
        candidate = request(); candidate['generation'] = 2; candidate['disks'][0]['initialize'] = False
        candidate['disks'].append({'id': 'nhsecond', 'device': '/dev/vdc', 'allocationBytes': 16 * GIB, 'initialize': True})
        new = {**candidate['disks'][1], 'initialize': False,
               'partitionUuid': 'd74a6325-dbbf-40de-bc98-4fe87b7d6004', 'pvUuid': 'bbbbbb-cccc-dddd-eeee-ffff-gggg-hhhhhh'}
        members = [old['disks'][0], new]
        pending = {**copy.deepcopy(candidate), 'requestDigest': storage.fingerprint(candidate),
                   'oldState': old, 'members': members, 'filesystemUuid': FILESYSTEM}
        journal = self.config / 'storage-pending.json'; journal.write_text(json.dumps(pending))
        second = owned_disk('/dev/vdc'); second['partitions'][0].update(partitionUuid=new['partitionUuid'], pvUuid=new['pvUuid'])
        vg_name, lv_name = storage.names(old); owner_tags = ','.join(sorted(storage.tags(old)))
        def execute(*args, **kwargs):
            self.commands.append(args)
            if args[:2] == ('systemctl', 'show'):return 'inactive\n'
            if args[0] == 'pvs':
                return json.dumps({'report': [{'pv': [{'pv_name': disk['device'] + '1', 'pv_uuid': disk['pvUuid'], 'vg_name': vg_name} for disk in members]}]})
            if args[0] == 'vgs':
                return json.dumps({'report': [{'vg': [{'vg_name': vg_name, 'vg_uuid': old['vgUuid'], 'vg_tags': owner_tags, 'vg_free_count': '0'}]}]})
            if args[0] == 'lvs':
                return json.dumps({'report': [{'lv': [{'vg_name': vg_name, 'lv_name': lv_name, 'lv_uuid': old['lvUuid'], 'lv_tags': owner_tags, 'segtype': 'linear'}]}]})
            if args[0] == 'blkid':return f'TYPE=ext4\nUUID={FILESYSTEM}\n'
            return ''
        mount = {'target': '/var/lib/nodeharbor/storage', 'fstype': 'ext4', 'uuid': FILESYSTEM, 'options': 'rw'}
        with patch.object(storage, 'inspect_devices', return_value=[owned_disk(), second]), \
             patch.object(storage, 'pool_mount', return_value=mount), \
             patch.object(storage, 'check_pool', side_effect=lambda root, execute: storage.load_state(root)):
            result = storage.activate_pool(root=self.root, execute=execute)
        self.assertEqual(result['generation'], 2)
        self.assertEqual(result['filesystemUuid'], FILESYSTEM)
        self.assertTrue(result['migrationComplete'])
        self.assertEqual({disk['pvUuid'] for disk in result['disks']}, {PV, new['pvUuid']})
        self.assertFalse(journal.exists())
        self.assertFalse(any(command[0] in {'sfdisk', 'pvcreate', 'mkfs.ext4', 'vgcreate', 'vgextend', 'lvcreate'} for command in self.commands))

    def test_boot_recovery_rejects_a_changed_or_foreign_pending_request(self):
        self.save_state()
        original = request(); original['generation'] = 2; original['disks'][0]['initialize'] = False
        for field, value in [('deviceId', 'e9c48768-5b5d-4451-bff4-f8e958529349'), ('generation', 3)]:
            pending = {**copy.deepcopy(original), 'requestDigest': storage.fingerprint(original),
                       'oldState': previous(), 'members': previous()['disks'], 'filesystemUuid': FILESYSTEM}
            pending[field] = value
            (self.config / 'storage-pending.json').write_text(json.dumps(pending))
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'owner|device|digest|request'):
                storage.activate_pool(root=self.root, execute=self.execute)
        self.assertFalse(any(command[0] in {'sfdisk', 'pvcreate', 'pvresize', 'lvextend', 'resize2fs', 'lvchange'} for command in self.commands))

    def test_boot_recovery_requires_the_worker_to_be_stopped(self):
        self.save_state()
        candidate = request(); candidate['generation'] = 2; candidate['disks'][0]['initialize'] = False
        pending = {**candidate, 'requestDigest': storage.fingerprint(candidate),
                   'oldState': previous(), 'members': previous()['disks'], 'filesystemUuid': FILESYSTEM}
        (self.config / 'storage-pending.json').write_text(json.dumps(pending))
        with self.assertRaisesRegex(ValueError, 'stop|running|active'):
            storage.activate_pool(root=self.root, execute=lambda *args, **kwargs: 'active\n')

    def test_apply_rejects_foreign_or_nonblank_disks_before_running_a_write_command(self):
        for changed in [{'signatures': ['ext4']}, {'containsRoot': True}]:
            disk = empty_disk(); disk.update(changed)
            with patch.object(storage, 'inspect_devices', return_value=[disk]):
                with self.assertRaises(ValueError):
                    storage.apply_request(request(), root=self.root, execute=self.execute)
            destructive = {'sfdisk', 'pvcreate', 'vgcreate', 'vgextend', 'lvcreate', 'lvextend', 'mkfs.ext4', 'resize2fs'}
            self.assertFalse(any(command[0] in destructive for command in self.commands))
            self.assertFalse((self.config / 'storage-state.json').exists())

    def test_boot_activation_of_a_missing_member_never_formats_or_activates_partially(self):
        self.save_state()
        with patch.object(storage, 'inspect_devices', return_value=[]):
            with self.assertRaises(ValueError):
                storage.activate_pool(root=self.root, execute=self.execute)
        self.assertFalse(any(command[0] in {'sfdisk', 'pvcreate', 'mkfs.ext4', 'lvchange', 'vgchange', 'mount'} for command in self.commands))

    def test_readiness_rejects_a_root_disk_mounted_in_place_of_the_owned_pool(self):
        self.save_state()
        def execute(*args, **kwargs):
            if args[0] == 'findmnt':return json.dumps({'filesystems': [{'target': '/var/lib/nodeharbor/storage', 'fstype': 'ext4', 'uuid': POOL, 'options': 'rw'}]})
            return self.execute(*args, **kwargs)
        with patch.object(storage, 'inspect_devices', return_value=[owned_disk()]):
            with self.assertRaises(ValueError):
                storage.check_pool(root=self.root, execute=execute)

    def test_migration_copies_and_verifies_existing_data_without_removing_sources(self):
        state = previous(); state['migrationComplete'] = False; self.save_state(state)
        sources = ['var/lib/rancher/k3s', 'var/lib/kubelet', 'var/log/pods']
        for source in sources:
            path = self.root / source; path.mkdir(parents=True); (path / 'keep').write_text(source)
        with patch.object(storage, 'check_pool', return_value=state):
            storage.migrate_legacy(root=self.root, execute=self.execute)
        for source in sources:self.assertEqual((self.root / source / 'keep').read_text(), source)
        copies = [command for command in self.commands if command[0] == 'rsync' and '--dry-run' not in command]
        verifies = [command for command in self.commands if command[0] == 'rsync' and '--dry-run' in command]
        self.assertEqual(len(copies), 3); self.assertEqual(len(verifies), 3)
        for command in copies:self.assertIn('--numeric-ids', command); self.assertIn('-aHAXS', command)
        for command in verifies:self.assertIn('--checksum', command)
        self.assertTrue(json.loads((self.config / 'storage-state.json').read_text())['migrationComplete'])

    def test_failed_copy_or_verification_never_marks_legacy_data_migrated(self):
        state = previous(); state['migrationComplete'] = False; self.save_state(state)
        source = self.root / 'var/lib/rancher/k3s'; source.mkdir(parents=True); (source / 'keep').write_text('original')
        for failure in ['copy', 'verification']:
            def execute(*args, **kwargs):
                if args[0] == 'rsync':
                    if failure == 'copy':raise RuntimeError('copy interrupted')
                    if '--dry-run' in args:return '>f.s....... keep\n'
                return self.execute(*args, **kwargs)
            with self.subTest(failure=failure), patch.object(storage, 'check_pool', return_value=state):
                with self.assertRaises((RuntimeError, ValueError)):
                    storage.migrate_legacy(root=self.root, execute=execute)
            self.assertEqual((source / 'keep').read_text(), 'original')
            self.assertFalse(json.loads((self.config / 'storage-state.json').read_text())['migrationComplete'])

    def test_running_worker_cannot_be_migrated(self):
        state = previous(); state['migrationComplete'] = False; self.save_state(state)
        with patch.object(storage, 'check_pool', return_value=state):
            with self.assertRaisesRegex(ValueError, 'stop|running|active'):
                storage.migrate_legacy(root=self.root, execute=lambda *args, **kwargs: 'active\n')
        self.assertFalse(json.loads((self.config / 'storage-state.json').read_text())['migrationComplete'])

    def machine_cache(self):
        root = self.root.resolve()
        return root / 'var/lib/harbor-build', root / 'var/lib/nodeharbor/storage/harbor-build'

    def activate(self, check=None):
        mount = {'target': '/var/lib/nodeharbor/storage', 'fstype': 'ext4', 'uuid': FILESYSTEM, 'options': 'rw'}
        with patch.object(storage, 'inspect_devices', return_value=[owned_disk()]), \
             patch.object(storage, 'validate_lvm'), patch.object(storage, 'pool_mount', return_value=mount), \
             patch.object(storage, 'check_pool', side_effect=check or (lambda root, execute: previous())):
            return storage.activate_pool(root=self.root, execute=self.execute)

    def test_activation_links_the_machine_cache_into_the_owned_pool(self):
        self.save_state(); link, directory = self.machine_cache()
        def check(root, execute):
            self.assertFalse(link.is_symlink(), 'The link waits for the verified pool mount')
            return previous()
        self.activate(check)
        self.assertTrue((directory / 'cache').is_dir())
        self.assertEqual((directory / 'cache').stat().st_mode & 0o777, 0o700)
        self.assertTrue(link.is_symlink())
        self.assertEqual(os.readlink(link), '/var/lib/nodeharbor/storage/harbor-build')
        self.assertIn(('chown', '65534:65534', str(directory), str(directory / 'cache')), self.commands)

    def test_a_failed_pool_check_leaves_no_machine_cache_link(self):
        self.save_state(); link, _ = self.machine_cache()
        def check(root, execute):raise ValueError('The mounted storage pool has changed identity or is read-only')
        with self.assertRaises(ValueError):self.activate(check)
        self.assertFalse(link.is_symlink())
        self.assertFalse(any(command[0] == 'chown' for command in self.commands))

    def test_a_failed_machine_cache_link_still_returns_the_verified_pool(self):
        self.save_state(); link, _ = self.machine_cache()
        with patch.object(storage, 'link_machine_cache', side_effect=OSError('read-only pool')), \
             patch.object(storage.sys, 'stderr', io.StringIO()) as stderr:
            self.assertEqual(self.activate(), previous())
        self.assertIn('cache link skipped', stderr.getvalue()); self.assertIn('read-only pool', stderr.getvalue())
        self.assertFalse(link.is_symlink())

    def test_an_unrelated_file_at_the_machine_cache_path_is_replaced_by_the_pool_link(self):
        link, directory = self.machine_cache()
        link.parent.mkdir(parents=True); link.write_text('not a cache')
        storage.link_machine_cache(root=self.root, execute=self.execute)
        self.assertTrue(link.is_symlink())
        self.assertEqual(os.readlink(link), '/var/lib/nodeharbor/storage/harbor-build')
        self.assertTrue((directory / 'cache').is_dir())

    def test_a_system_disk_machine_cache_is_replaced_by_the_pool_link(self):
        link, directory = self.machine_cache()
        stale = link / 'cache'; stale.mkdir(parents=True); (stale / 'blocks').write_bytes(b'old cache')
        with patch.object(storage.shutil, 'rmtree', wraps=shutil.rmtree) as rmtree:
            storage.link_machine_cache(root=self.root, execute=self.execute)
        rmtree.assert_called_once_with(link)
        self.assertTrue(link.is_symlink())
        self.assertEqual(os.readlink(link), '/var/lib/nodeharbor/storage/harbor-build')
        self.assertEqual(list((directory / 'cache').iterdir()), [])

    def test_an_existing_machine_cache_link_is_left_untouched(self):
        link, directory = self.machine_cache()
        link.parent.mkdir(parents=True); link.symlink_to('/var/lib/nodeharbor/storage/harbor-build')
        before = os.lstat(link)
        with patch.object(storage.shutil, 'rmtree', wraps=shutil.rmtree) as rmtree:
            storage.link_machine_cache(root=self.root, execute=self.execute)
        rmtree.assert_not_called()
        self.assertEqual(os.lstat(link).st_ino, before.st_ino)
        self.assertEqual(os.readlink(link), '/var/lib/nodeharbor/storage/harbor-build')
        self.assertTrue((directory / 'cache').is_dir())

    def test_a_machine_cache_link_elsewhere_is_replaced_without_following_it(self):
        link, _ = self.machine_cache()
        elsewhere = self.root.resolve() / 'elsewhere'; elsewhere.mkdir(); (elsewhere / 'keep').write_text('keep')
        link.parent.mkdir(parents=True); link.symlink_to(elsewhere)
        with patch.object(storage.shutil, 'rmtree', wraps=shutil.rmtree) as rmtree:
            storage.link_machine_cache(root=self.root, execute=self.execute)
        rmtree.assert_not_called()
        self.assertEqual(os.readlink(link), '/var/lib/nodeharbor/storage/harbor-build')
        self.assertEqual((elsewhere / 'keep').read_text(), 'keep')

    def test_retire_removes_the_machine_cache_link_with_the_pool(self):
        self.save_state(); link, directory = self.machine_cache()
        (directory / 'cache').mkdir(parents=True); (directory / 'cache/blocks').write_bytes(b'cache')
        link.symlink_to('/var/lib/nodeharbor/storage/harbor-build')
        self.assertEqual(storage.retire_pool({'poolId': POOL}, root=self.root, execute=self.execute), {'ok': True})
        self.assertFalse(link.is_symlink()); self.assertFalse(link.exists())
        self.assertTrue((directory / 'cache/blocks').exists(), 'The pool is unmounted, never deleted through the link')
        self.assertFalse((self.config / 'storage-state.json').exists())
        self.assertIn(('systemctl', 'disable', '--now', 'nodeharbor-storage.service'), self.commands)

    def test_retire_removes_a_system_disk_machine_cache(self):
        self.save_state(); link, _ = self.machine_cache()
        (link / 'cache').mkdir(parents=True); (link / 'cache/blocks').write_bytes(b'cache')
        storage.retire_pool({'poolId': POOL}, root=self.root, execute=self.execute)
        self.assertFalse(link.exists()); self.assertFalse(link.is_symlink())
        self.assertFalse((self.config / 'storage-state.json').exists())

    def test_retire_removes_an_unrelated_file_at_the_machine_cache_path(self):
        self.save_state(); link, _ = self.machine_cache()
        link.parent.mkdir(parents=True); link.write_text('not a cache')
        self.assertEqual(storage.retire_pool({'poolId': POOL}, root=self.root, execute=self.execute), {'ok': True})
        self.assertFalse(link.exists()); self.assertFalse(link.is_symlink())
        storage.link_machine_cache(root=self.root, execute=self.execute)
        self.assertTrue(link.is_symlink(), 'A later activation must be able to make the link')

    def test_retire_of_another_pool_touches_nothing(self):
        self.save_state(); link, _ = self.machine_cache()
        link.parent.mkdir(parents=True); link.symlink_to('/var/lib/nodeharbor/storage/harbor-build')
        with self.assertRaisesRegex(ValueError, 'pool'):
            storage.retire_pool({'poolId': OWNER}, root=self.root, execute=self.execute)
        self.assertTrue(link.is_symlink())
        self.assertTrue((self.config / 'storage-state.json').exists())
        self.assertFalse(any(command[0] in {'umount', 'rm'} or command[:2] == ('systemctl', 'disable') for command in self.commands))

    def test_retire_requires_the_worker_to_be_stopped(self):
        self.save_state(); link, _ = self.machine_cache()
        link.parent.mkdir(parents=True); link.symlink_to('/var/lib/nodeharbor/storage/harbor-build')
        with self.assertRaisesRegex(ValueError, 'stop|running|active'):
            storage.retire_pool({'poolId': POOL}, root=self.root, execute=lambda *args, **kwargs: 'active\n')
        self.assertTrue(link.is_symlink())
        self.assertTrue((self.config / 'storage-state.json').exists())

    def test_restored_marks_the_matching_replacement_pool_migrated(self):
        with self.assertRaisesRegex(ValueError, 'unavailable'):
            storage.restore_pool({'poolId': POOL}, root=self.root, execute=self.execute)
        state = previous(); state['migrationComplete'] = False; self.save_state(state)
        with patch.object(storage, 'check_pool', return_value=state) as check:
            self.assertEqual(storage.restore_pool({'poolId': POOL}, root=self.root, execute=self.execute), {'ok': True})
        check.assert_called_once()
        self.assertTrue(json.loads((self.config / 'storage-state.json').read_text())['migrationComplete'])
