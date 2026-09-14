import subprocess
import unittest
from unittest.mock import Mock

import native_storage_kubernetes as proof


class NativeStorageProofContract(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
