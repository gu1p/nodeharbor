#!/usr/bin/env python3
"""Run reproducible controller/agent scenarios without a private deployment.

Kubernetes, NetBird, VM execution and observation time are explicit test doubles.
Authentication, HTTP provisioning, SQLite persistence, storage validation, owner
controls and qualification decisions execute the production code. Device/runtime
qualification is recorded separately by qualify_android.py and native tests.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import uuid
from release import ACCEPTANCE_SCENARIOS, android_version_code, validate_reproducible_acceptance

ROOT = Path(__file__).resolve().parents[1]


def commands(name):
    cargo = ['cargo', 'test', '--locked']
    if name == 'infrastructure_http_contracts':
        return [[*cargo, '-p', 'nodeharbor-controller', '--test', 'provisioning',
                 '--test', 'netbird_groups', '--test', 'network_probe', '--test', 'health_contract']]
    result = [[*cargo, '-p', 'nodeharbor-controller', '--test', 'reproducible_acceptance', name, '--', '--exact']]
    if name == 'preparation_failure_recovery':
        result.append([*cargo, '-p', 'nodeharbor-agent', '--test', 'operation_cancellation',
                       '--test', 'preparation_lease', '--test', 'vm_recovery'])
    if name == 'missing_storage_and_restart':
        result.append([*cargo, '-p', 'nodeharbor-agent', '--test', 'storage_changes',
                       '--test', 'lima_storage_recovery', '--test', 'storage_lifecycle_integration',
                       '--test', 'storage_disappearance', '--test', 'storage_locations'])
    return result


def passed_tests(output):
    summaries = re.findall(r'^test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;', output, re.MULTILINE)
    if not summaries or any(state != 'ok' or int(failed) or int(ignored) or not int(passed)
                            for state, passed, failed, ignored in summaries):
        raise ValueError('An acceptance command failed, skipped tests, or executed no tests')
    return sum(int(passed) for _, passed, _, _ in summaries)


def run(output, version, commit, allow_dirty=False):
    android_version_code(version)
    actual = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True))
    if not re.fullmatch('[0-9a-f]{40}', commit) or actual != commit or (dirty and not allow_dirty):
        raise ValueError('Acceptance requires the exact clean commit; --allow-dirty is for local development only')
    if output.exists(): raise ValueError('Use a new evidence path so a failed run cannot leave stale success evidence')
    evidence = dict(schemaVersion=1, version=version, commit=commit, dirtySource=dirty,
                    infrastructure='simulated', nativeRuntimeTested=False, scenarios={})
    environment = {k: v for k, v in os.environ.items() if k not in
                   ['NODEHARBOR_ANDROID_QUALIFICATION_CONFIG', 'KUBECONFIG']}
    for name in ACCEPTANCE_SCENARIOS:
        print('Acceptance: ' + name, flush=True)
        count = 0
        for command in commands(name):
            result = subprocess.run(command, cwd=ROOT, env=environment, capture_output=True, text=True, timeout=900)
            if result.returncode:
                print(result.stdout + result.stderr, flush=True)
                raise ValueError('Acceptance failed: ' + name)
            count += passed_tests(result.stdout)
        evidence['scenarios'][name] = dict(passed=True, tests=count)
    if not dirty: validate_reproducible_acceptance(evidence, version, commit)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', dir=output.parent, prefix='.acceptance-', delete=False) as file:
        pending = Path(file.name)
        try:
            json.dump(evidence, file, indent=2); file.write('\n'); file.flush()
            # Exclusive creation prevents an overlapping run from replacing evidence.
            os.link(pending, output)
        finally: pending.unlink(missing_ok=True)
    print('Reproducible scenarios passed; infrastructure was simulated. Evidence: ' + str(output), flush=True)
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--list', action='store_true', help='Print the scenario catalog without running tools')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--version'); parser.add_argument('--commit')
    parser.add_argument('--allow-dirty', action='store_true', help='Write development evidence that publication rejects')
    args = parser.parse_args()
    if args.list:
        print(json.dumps(dict(infrastructure='simulated', requiresPrivateDeployment=False, scenarios=ACCEPTANCE_SCENARIOS)))
        return
    commit = args.commit or subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    version = args.version or subprocess.check_output([os.sys.executable, str(ROOT / 'scripts/release.py'), 'version'], cwd=ROOT, text=True).strip()
    output = args.output or ROOT / '.local/acceptance' / (uuid.uuid4().hex + '.json')
    run(output, version, commit, args.allow_dirty)


if __name__ == '__main__': main()
