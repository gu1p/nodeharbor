import importlib.util
import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('android_control', ROOT / 'guest/android_control.py')
control = importlib.util.module_from_spec(spec)
watch_spec = importlib.util.spec_from_file_location('watchdog', ROOT / 'guest/watchdog.py')
watchdog = importlib.util.module_from_spec(watch_spec)
watch_spec.loader.exec_module(watchdog)
with mock.patch.dict('sys.modules', {'watchdog': watchdog}): spec.loader.exec_module(control)
OWNER = '9511182e-9c48-4d20-a15b-1da8bb441386'


class AndroidGuestControlContract(unittest.TestCase):
    def test_early_owner_renewal_is_atomic_without_starting_another_interpreter(self):
        with tempfile.TemporaryDirectory() as directory:
            lease = Path(directory) / 'run/lease'
            with mock.patch.object(watchdog, 'LEASE', lease), mock.patch.object(watchdog, 'uptime', return_value=42.5), \
                    mock.patch.object(control.subprocess, 'run', side_effect=AssertionError('No subprocess during owner renewal')):
                control.renew_owner_lease()
                self.assertEqual(lease.read_text(), '42.5')
                self.assertFalse(lease.with_suffix('.new').exists())

    def test_diagnostics_are_fixed_bounded_and_never_include_command_payloads(self):
        with mock.patch('builtins.print') as output:
            for _ in range(100): control.lifecycle_event('poweroff-received')
            with self.assertRaises(ValueError): control.lifecycle_event('secret payload')
        output.assert_called_once_with('NODEHARBOR_CONTROL poweroff-received', flush=True)

    def test_status_identifies_the_control_revision_for_existing_disk_upgrades(self):
        guest = control.Guest(OWNER)
        guest.configured = False
        # A unit activation and its code must agree before readiness. Replacing
        # the Python file alone does not update a running process's unit ordering.
        with mock.patch.dict(control.os.environ, {'NODEHARBOR_CONTROL_UNIT_REVISION': '4'}):
            self.assertEqual(guest.status()['controlRevision'], '4')

    def test_new_code_under_a_legacy_unit_answers_without_advertising_readiness(self):
        guest = control.Guest(OWNER)
        for configured in [False, True]:
            guest.configured = configured
            for revision in ['', '2', '3', 'unexpected']:
                with self.subTest(revision=revision, configured=configured), mock.patch.dict(control.os.environ, {'NODEHARBOR_CONTROL_UNIT_REVISION': revision}), \
                        mock.patch.object(control.subprocess, 'run', side_effect=AssertionError('Early status must not block owner leases')):
                    status = guest.status()
                    self.assertEqual(status['controlRevision'], '')
                    self.assertEqual(status['configured'], configured)
                    self.assertFalse(status['running'])
                    self.assertIsNone(status['pods'])

    def test_an_unconfigured_guest_answers_before_system_services_are_ready(self):
        guest = control.Guest(OWNER)
        guest.configured = False
        with mock.patch.object(control.subprocess, 'run', side_effect=control.subprocess.TimeoutExpired('systemctl', 10)) as command:
            status = guest.status()
        command.assert_not_called()
        self.assertFalse(status['running'])
        self.assertIsNone(status['pods'])

    def test_unavailable_service_status_never_claims_a_running_or_empty_worker(self):
        guest = control.Guest(OWNER)
        guest.configured = True
        with mock.patch.dict(control.os.environ, {'NODEHARBOR_CONTROL_UNIT_REVISION': '4'}), \
                mock.patch.object(control.subprocess, 'run', side_effect=control.subprocess.TimeoutExpired('systemctl', 10)) as command:
            status = guest.status()
        command.assert_called_once()
        self.assertTrue(status['configured'])
        self.assertFalse(status['running'])
        self.assertIsNone(status['pods'])

    def test_service_readiness_is_reported_through_the_guest_systemd_socket(self):
        socket = mock.MagicMock()
        socket.__enter__.return_value = socket
        with mock.patch.dict(control.os.environ, {'NOTIFY_SOCKET':'@nodeharbor-test'}), mock.patch.object(control, 'socket', create=True) as api:
            api.socket.return_value = socket
            control.notify_ready()
        socket.connect.assert_called_once_with('\0nodeharbor-test')
        socket.sendall.assert_called_once_with(b'READY=1')

    def test_timed_out_configuration_kills_and_reaps_its_child(self):
        child = mock.MagicMock()
        child.__enter__.return_value = child
        child.communicate.side_effect = control.subprocess.TimeoutExpired('configure', 1200)
        with mock.patch.object(control.subprocess, 'Popen', return_value=child):
            guest = control.Guest(OWNER)
            guest._configure({'deviceId': OWNER})
        child.kill.assert_called_once()
        child.wait.assert_called_once()
        self.assertFalse(guest.preparing)
        self.assertTrue(guest.error)

    def test_control_has_only_fixed_owner_scoped_operations(self):
        for command in ['status', 'lease', 'poweroff']:
            value = {'id': 1, 'deviceId': OWNER, 'command': command}
            self.assertEqual(control.validate_request(value, OWNER), value)
        for values in [{'command': 'exec'}, {'deviceId': 'other'}, {'arguments': ['sh']}, {'id': -1}]:
            request = dict(id=1, deviceId=OWNER, command='status') | values
            with self.subTest(values=values), self.assertRaises(ValueError):
                control.validate_request(request, OWNER)

    def test_bootstrap_cannot_configure_a_different_worker(self):
        request = dict(id=2, deviceId=OWNER, command='configure', bootstrap={'deviceId': OWNER})
        control.validate_request(request, OWNER)
        request['bootstrap']['deviceId'] = 'different'
        with self.assertRaises(ValueError): control.validate_request(request, OWNER)

    def test_protocol_reads_complete_bounded_frames_and_refuses_truncation(self):
        value = dict(id=1, deviceId=OWNER, command='status')
        stream = io.BytesIO()
        control.write_frame(stream, value)
        stream.seek(0)
        self.assertEqual(control.read_frame(stream), value)
        for data in [b'\x00', struct.pack('>I', 1024 * 1024 + 1), struct.pack('>I', 5) + b'{}', struct.pack('>I', 2) + b'[]']:
            with self.subTest(data=data), self.assertRaises((ValueError, EOFError)):
                control.read_frame(io.BytesIO(data))

    def test_unknown_or_unready_workload_inventory_cannot_claim_to_be_empty(self):
        pods = {'items': [{'state': 'SANDBOX_READY', 'metadata': {'uid': OWNER, 'namespace': 'jobs'}},
                          {'state': 'SANDBOX_NOTREADY', 'metadata': {'uid': OWNER, 'namespace': 'kube-system'}}]}
        self.assertEqual(len(control.pod_inventory(json.dumps(pods))), 2)
        for value in [{}, {'items': [{'state': 'unknown', 'metadata': {'uid': OWNER, 'namespace': 'jobs'}}]}]:
            with self.assertRaises(ValueError): control.pod_inventory(json.dumps(value))


if __name__ == '__main__': unittest.main()
