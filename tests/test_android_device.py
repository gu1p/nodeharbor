from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import test_android as device


class AndroidDeviceProtocol(unittest.TestCase):
    def test_status_bundles_can_follow_the_junit_class_prefix(self):
        output = 'app.Contract:INSTRUMENTATION_STATUS: vpnFingerprint=abc\nINSTRUMENTATION_STATUS_CODE: 2\n.\nOK (1 test)\n'
        self.assertEqual(device.instrumentation_values(output), {'vpnFingerprint':'abc'})

    def test_missing_failed_or_empty_test_runs_never_qualify_an_apk(self):
        for output in ['', 'OK (0 tests)', 'OK (1 test)\nFAILURES!!!', 'INSTRUMENTATION_FAILED: target missing']:
            with self.assertRaises(ValueError): device.instrumentation_values(output)
