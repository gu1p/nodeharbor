import copy
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('acceptance_release', ROOT / 'scripts/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
SCENARIOS = ['enrollment_authentication', 'preparation_failure_recovery',
             'qualification_and_owner_controls', 'missing_storage_and_restart',
             'revocation_and_reenrollment', 'infrastructure_http_contracts']


def report():
    return dict(schemaVersion=1, version='0.2.76', commit='a' * 40, dirtySource=False,
                infrastructure='simulated', nativeRuntimeTested=False,
                scenarios={name: dict(passed=True, tests=1) for name in SCENARIOS})


class AcceptanceEvidenceContract(unittest.TestCase):
    def test_source_bound_evidence_requires_every_executed_scenario(self):
        good = report()
        release.validate_reproducible_acceptance(good, '0.2.76', 'a' * 40)
        invalid = [dict(good, commit='b' * 40), dict(good, dirtySource=True),
                   dict(good, infrastructure='real'), dict(good, schemaVersion=0)]
        for name in SCENARIOS:
            for failure in [None, dict(passed=False, tests=1), dict(passed=True, tests=0)]:
                candidate = copy.deepcopy(good)
                if failure is None: del candidate['scenarios'][name]
                else: candidate['scenarios'][name] = failure
                invalid.append(candidate)
        for candidate in invalid:
            with self.subTest(candidate=candidate), self.assertRaises(ValueError):
                release.validate_reproducible_acceptance(candidate, '0.2.76', 'a' * 40)

    def test_device_results_and_simulations_are_required_without_claiming_a_live_cluster(self):
        evidence = dict(version='0.2.76', commit='a' * 40, apkSha256='b' * 64,
                        certificateSha256='c' * 64, signedRelease=True, physical=True,
                        apiLevels=[33, 36, 37], ownerControlsPassed=True, vpnPreserved=True,
                        upgradePassed=True, upgradeFromVersion='0.2.75', nativeContractsPassed=True,
                        qualificationMode='reproducible', acceptance=report())
        release.validate_android_qualification(evidence, '0.2.76', 'a' * 40, 'b' * 64)
        for changes in [dict(nativeContractsPassed=False), dict(physical=False),
                        dict(acceptance={}), dict(ciQualified=True), dict(arm64JobSucceeded=True)]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                release.validate_android_qualification(dict(evidence, **changes), '0.2.76', 'a' * 40, 'b' * 64)
