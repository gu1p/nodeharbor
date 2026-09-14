import copy
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('android_release_contract', ROOT / 'scripts/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class AndroidReleaseContract(unittest.TestCase):
    def evidence(self):
        return dict(version='0.1.12', commit='a' * 40, apkSha256='b' * 64,
                    certificateSha256='c' * 64, signedRelease=True, physical=True,
                    apiLevels=[33, 36, 37], ownerControlsPassed=True, vpnPreserved=True,
                    ciQualified=True, arm64JobSucceeded=True, upgradePassed=True, upgradeFromVersion='0.1.11')

    def test_an_apk_needs_matching_real_device_and_fleet_evidence(self):
        evidence = self.evidence()
        release.validate_android_qualification(evidence, '0.1.12', 'a' * 40, 'b' * 64)
        for field in ['signedRelease', 'physical', 'ownerControlsPassed', 'vpnPreserved', 'ciQualified', 'arm64JobSucceeded']:
            invalid = dict(evidence, **{field: False})
            with self.subTest(field=field), self.assertRaises(ValueError):
                release.validate_android_qualification(invalid, '0.1.12', 'a' * 40, 'b' * 64)
        for field, value in [('apiLevels', [33]), ('commit', 'd' * 40), ('apkSha256', 'e' * 64), ('version', '0.1.11'), ('certificateSha256', '')]:
            invalid = dict(evidence, **{field: value})
            with self.subTest(field=field), self.assertRaises(ValueError):
                release.validate_android_qualification(invalid, '0.1.12', 'a' * 40, 'b' * 64)

    def test_release_requires_a_verified_signed_upgrade_from_a_lower_version(self):
        evidence = dict(self.evidence(), upgradePassed=True, upgradeFromVersion='0.1.11')
        release.validate_android_qualification(evidence, '0.1.12', 'a'*40, 'b'*64)
        for invalid in [dict(evidence, upgradePassed=False), dict(evidence, upgradeFromVersion='0.1.12'), dict(evidence, upgradeFromVersion='0.1.13')]:
            with self.assertRaises(ValueError):
                release.validate_android_qualification(invalid, '0.1.12', 'a'*40, 'b'*64)

    def test_android_version_codes_are_ordered_and_fit_the_platform_limit(self):
        self.assertLess(release.android_version_code('0.1.12'), release.android_version_code('0.2.0'))
        self.assertEqual(release.android_version_code('0.1.12'), 1_000_012)
        for version in ['0.1.0-development', '0.1.1000000', '99.0.0', '1.100.0']:
            with self.assertRaises(ValueError): release.android_version_code(version)

    def test_publication_requires_the_android_qualification_job(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertIn('android-qualification:', workflow)
        self.assertIn('needs: [identity, native, container-publish, android-qualification]', workflow)
        self.assertIn('python scripts/qualify_android.py', workflow)
