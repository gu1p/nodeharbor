import subprocess
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import native_storage_kubernetes as proof


class NativeStorageProofContract(unittest.TestCase):
    @staticmethod
    def node(boot):
        return {'status': {'nodeInfo': {'bootID': boot},
                           'conditions': [{'type': 'Ready', 'status': 'True'}]}}

    def test_restart_waits_for_current_boot_and_a_live_kubelet(self):
        old, current = self.node('previous-boot'), self.node('current-boot')
        fixture = Mock()
        fixture.shell.return_value = 'current-boot\n'
        fixture.kubectl.side_effect = [json.dumps(old), json.dumps(current),
            RuntimeError('Kubelet tunnel is restarting'), json.dumps(current), 'ok\n']
        with patch.object(proof.time, 'sleep'):
            self.assertEqual(proof.wait_ready(fixture), current)
        self.assertEqual(fixture.kubectl.call_count, 5)

    def test_stale_ready_state_cannot_outlive_the_restart_deadline(self):
        fixture = Mock()
        fixture.shell.return_value = 'current-boot\n'
        fixture.kubectl.return_value = json.dumps(self.node('previous-boot'))
        with patch.object(proof.time, 'monotonic', side_effect=[0, 0, 181]), \
             patch.object(proof.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'did not become Ready'):
                proof.wait_ready(fixture)

    def test_size_verification_does_not_stream_the_cross_disk_workload(self):
        def metadata_only(*arguments):
            command = arguments[arguments.index('--') + 1:]
            if command != ('stat', '-c', '%s', '/scratch/payload'):
                raise subprocess.TimeoutExpired(command, 120)
            return '10737418240\n'

        fixture = Mock()
        fixture.kubectl.side_effect = metadata_only
        self.assertEqual(proof.payload_size(fixture), 10 * 1024 ** 3)
        self.assertEqual(fixture.kubectl.call_count, 1)

    def test_empty_payload_is_reported_as_empty(self):
        fixture = Mock()
        fixture.kubectl.return_value = '0\n'
        self.assertEqual(proof.payload_size(fixture), 0)

    def test_large_workload_verification_budget_accounts_for_volume_throughput(self):
        self.assertGreaterEqual(proof.payload_timeout(10 * 1024 ** 3), 640)
        self.assertGreater(proof.payload_timeout(20 * 1024 ** 3),
                           proof.payload_timeout(10 * 1024 ** 3))
        self.assertGreaterEqual(proof.payload_timeout(0), 180)

    @unittest.skipIf(os.name == 'nt', 'The native Lima guest uses a POSIX shell')
    def test_restart_clears_readiness_before_verifying_existing_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'ready').touch()
            (root / 'payload.sha256').touch()
            command = root / 'sha256sum'
            command.write_text('#!/bin/sh\n[ ! -e ./ready ] || exit 99\nexit 1\n')
            command.chmod(0o700)
            result = subprocess.run(['sh', '-c', proof.workload_script(1024 ** 3)
                                     .replace('/scratch/', './')], cwd=root,
                                    env={**os.environ, 'PATH': str(root) + os.pathsep + os.environ['PATH']},
                                    capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 1)
            self.assertFalse((root / 'ready').exists())


if __name__ == '__main__':
    unittest.main()
