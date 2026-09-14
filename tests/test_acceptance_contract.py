"""Contributor-facing acceptance commands must not require a private deployment."""
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]


class AcceptanceCommandContract(unittest.TestCase):
    def test_contributors_can_discover_scenarios_without_devices_or_credentials(self):
        environment = {k: v for k, v in os.environ.items()
                       if not k.startswith(('NODEHARBOR_', 'KUBE', 'GH_', 'GITHUB_'))}
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/acceptance.py'), '--list'],
                                env=environment, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        catalog = json.loads(result.stdout)
        self.assertEqual(catalog['infrastructure'], 'simulated')
        self.assertFalse(catalog['requiresPrivateDeployment'])
        self.assertIn('enrollment_authentication', catalog['scenarios'])
        self.assertIn('qualification_and_owner_controls', catalog['scenarios'])
        self.assertIn('missing_storage_and_restart', catalog['scenarios'])
        self.assertIn('revocation_and_reenrollment', catalog['scenarios'])

    def test_android_qualification_exposes_a_self_contained_default(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/qualify_android.py'), '--help'],
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        for option in ['--serial', '--acceptance-report', '--external-config']:
            self.assertIn(option, result.stdout)

    def test_release_runs_scenarios_on_public_ci_and_keeps_native_android_required(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertIn('acceptance:\n    needs: identity', workflow)
        self.assertIn('python scripts/acceptance.py', workflow)
        self.assertIn('needs: [identity, android, acceptance]', workflow)
        self.assertIn('--acceptance-report dist/reproducible-acceptance.json', workflow)
        self.assertIn('needs: [identity, native, container-publish, android-qualification]', workflow)
