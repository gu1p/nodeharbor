"""Release packages must run before they can enter the publication job."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
def module():
    spec=importlib.util.spec_from_file_location('package_smoke',ROOT/'scripts/package_smoke.py')
    value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value

class PackageSmoke(unittest.TestCase):
    def test_packaged_executable_must_report_the_exact_source_and_version_without_starting_a_worker(self):
        smoke=module()
        with patch.object(smoke.subprocess,'check_output',return_value='NodeHarbor 0.1.20 ('+'a'*40+')\n') as command:
            smoke.check_executable(Path('/package/nodeharbor'),'0.1.20','a'*40)
        self.assertEqual(command.call_args.args[0],[str(Path('/package/nodeharbor')),'--version'])
        with patch.object(smoke.subprocess,'check_output',return_value='NodeHarbor 0.1.200 ('+'a'*40+')\n'):
            with self.assertRaises(ValueError):smoke.check_executable(Path('/package/nodeharbor'),'0.1.20','a'*40)
        with patch.object(smoke.subprocess,'check_output',return_value='NodeHarbor 0.1.20 ('+'b'*40+')\n'):
            with self.assertRaises(ValueError):smoke.check_executable(Path('/package/nodeharbor'),'0.1.20','a'*40)

    def test_native_package_builds_run_installation_smoke_checks_before_returning(self):
        source=(ROOT/'scripts/build.py').read_text()
        self.assertIn('smoke_packages(',source)
        self.assertLess(source.index('    collect('),source.index('    smoke_packages('))
