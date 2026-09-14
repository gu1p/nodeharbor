#!/usr/bin/env python3
"""Qualify the exact signed APK on dedicated devices and the unchanged fleet scheduler."""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import time
import uuid
from build_android import certificate, previous_version, sdk_tool, verify_badging
from release import checksum, validate_android_qualification, android_version_code
from test_android import AndroidDevice, basic

CI_LABEL = 'nodeharbor.node-restriction.kubernetes.io/ci'


def ci_job(name, namespace, owner, image):
    uuid.UUID(owner)
    if not re.fullmatch(r'[A-Za-z0-9./:_-]+@sha256:[0-9a-f]{64}', image):
        raise ValueError('Use a pinned ARM64 CI image containing Python 3')
    code = "import platform,hashlib,sqlite3; assert platform.machine()=='aarch64'; " \
           "db=sqlite3.connect(':memory:'); db.execute('create table checks(value integer)'); " \
           "db.executemany('insert into checks values (?)',[(i,) for i in range(10000)]); " \
           "assert db.execute('select sum(value) from checks').fetchone()[0]==49995000; " \
           "assert len(hashlib.pbkdf2_hmac('sha256',b'nodeharbor',b'arm64',100000))==32; print('NODEHARBOR_ARM64_CI_OK')"
    pod = dict(restartPolicy='Never', automountServiceAccountToken=False,
               nodeSelector={'kubernetes.io/arch':'arm64', CI_LABEL:'true', 'nodeharbor.sikalio.dev/device':owner},
               tolerations=[dict(key='nodeharbor.sikalio.dev/contributed', operator='Equal', value='true', effect='NoSchedule')],
               securityContext=dict(runAsNonRoot=True, runAsUser=65532, seccompProfile={'type':'RuntimeDefault'}),
               containers=[dict(name='ci', image=image, command=['python3','-c',code],
                                resources={'requests':{'cpu':'100m','memory':'64Mi'}, 'limits':{'cpu':'500m','memory':'128Mi'}},
                                securityContext=dict(allowPrivilegeEscalation=False, readOnlyRootFilesystem=True, capabilities={'drop':['ALL']}))])
    return dict(apiVersion='batch/v1', kind='Job', metadata=dict(name=name, namespace=namespace),
                spec=dict(backoffLimit=0, activeDeadlineSeconds=300, ttlSecondsAfterFinished=3600,
                          template=dict(metadata=dict(labels={'app.kubernetes.io/name':'nodeharbor-android-qualification'}), spec=pod)))


def verify_job(job, pod, owner, image):
    status = job.get('status', {})
    expected = 'nodeharbor-' + uuid.UUID(owner).hex
    checks = [status.get('succeeded') == 1, not status.get('failed', 0),
              any(c.get('type') == 'Complete' and c.get('status') == 'True' for c in status.get('conditions', [])),
              pod.get('spec', {}).get('nodeName') == expected, pod.get('status', {}).get('phase') == 'Succeeded',
              any(r.get('uid') == job.get('metadata', {}).get('uid') and r.get('kind') == 'Job' for r in pod.get('metadata', {}).get('ownerReferences', [])),
              pod.get('spec', {}).get('containers') == [{'name':'ci', 'image':image}] or
              any(c.get('name') == 'ci' and c.get('image') == image for c in pod.get('spec', {}).get('containers', [])),
              any(c.get('name') == 'ci' and c.get('state', {}).get('terminated', {}).get('exitCode') == 0
                  for c in pod.get('status', {}).get('containerStatuses', []))]
    if not all(checks): raise ValueError('The real CI job did not complete on this owned ARM64 worker')


def signed_upgrade(device, previous_apk, previous_tests, apk, tests, version):
    identities = {certificate(path) for path in [previous_apk, previous_tests, apk, tests]}
    if len(identities) != 1: raise ValueError('Both signed versions and their instrumentation must use the persistent release certificate')
    device.install(previous_apk, previous_tests)
    before = device.vpn()
    marker = uuid.uuid4().hex
    device.instrument(['SignedUpdateContract#seed'], {'marker': marker})
    device.install(apk, tests)
    device.instrument(['SignedUpdateContract#verify'], {'marker': marker, 'versionCode': str(android_version_code(version))})
    if device.vpn() != before: raise ValueError('The VPN policy changed during the signed upgrade')


def qualify(folder, version, commit, config):
    prefix = f'nodeharbor-v{version}-aarch64-linux-android'
    manifest_path = folder / (prefix + '.json')
    manifest = json.loads(manifest_path.read_text())
    apk = folder / (prefix + '.apk')
    if manifest.get('signedRelease') is not True or manifest.get('dirtySource') is not False:
        raise ValueError('Physical release qualification requires a signed APK from clean source')
    if (manifest['version'], manifest['commit']) != (version, commit): raise ValueError('Mismatched APK source identity')
    if checksum(apk) != next(asset['sha256'] for asset in manifest['assets'] if asset['name'].endswith('.apk')):
        raise ValueError('The APK changed after packaging')
    if config.get('dedicatedDevices') is not True: raise ValueError('Use dedicated, authorized qualification devices')
    devices = [AndroidDevice(serial) for serial in config['devices']]
    if not {33, 36, 37}.issubset({device.api for device in devices}) or not any(device.physical for device in devices):
        raise ValueError('Connect dedicated ARM64 API 33, 36 and 37 devices, including a physical phone')
    previous = previous_version(version)
    baseline = folder / 'android-upgrade'
    previous_apk = baseline / f'nodeharbor-v{previous}-aarch64-linux-android.apk'
    previous_manifest = json.loads((baseline / f'nodeharbor-v{previous}-aarch64-linux-android.json').read_text())
    if (previous_manifest.get('version'), previous_manifest.get('commit'), previous_manifest.get('signedRelease'), previous_manifest.get('dirtySource')) != (previous, commit, True, False):
        raise ValueError('The upgrade baseline must identify the same clean tested source and a lower signed version')
    if checksum(previous_apk) != next(asset['sha256'] for asset in previous_manifest['assets'] if asset['name'].endswith('.apk')):
        raise ValueError('The upgrade baseline APK changed after packaging')
    for package, expected in [(previous_apk, previous), (apk, version)]:
        verify_badging(subprocess.check_output([sdk_tool('aapt2'), 'dump', 'badging', str(package)], text=True), expected)
    for device in devices:
        signed_upgrade(device, previous_apk, baseline / 'android-tests.apk', apk, folder / 'android-tests.apk', version)
    reports = [basic(device, apk, folder / 'android-tests.apk') for device in devices]
    phone = next(device for device in devices if device.physical)
    before = phone.vpn()
    provisioning = base64.b64encode(json.dumps({'controller':config['controller'],
        'enrollmentCode':config.get('enrollmentCode', ''), 'dedicatedDevice':True}).encode()).decode()
    owner = None
    created = False
    name = 'nodeharbor-android-' + uuid.uuid4().hex[:16]
    namespace = config['namespace']
    # The supplied kubeconfig/context is used as-is. This tool never changes it,
    # changes the VPN, removes quarantine, or assigns a pod directly to a node.
    command = ['kubectl', '--context', config['kubernetesContext'], '-n', namespace]
    def kube(args, body=None):
        result = subprocess.run(command + args, input=json.dumps(body) if body is not None else None,
                                capture_output=True, text=True, timeout=60)
        if result.returncode: raise ValueError('The fleet Kubernetes operation failed; private configuration was preserved')
        return result.stdout
    try:
        setup = phone.instrument(['FleetSetupContract'], {'qualification':provisioning}, timeout=4500)
        owner = setup.get('deviceId')
        uuid.UUID(owner)
        if setup.get('ciQualified') != 'true': raise ValueError('The fleet did not independently qualify the phone for CI')
        kube(['create', '-f', '-'], ci_job(name, namespace, owner, config['ciImage']))
        created = True
        end = time.monotonic() + 360
        while time.monotonic() < end:
            job = json.loads(kube(['get', 'job', name, '-o', 'json']))
            if job.get('status', {}).get('succeeded') == 1: break
            if job.get('status', {}).get('failed'): raise ValueError('The real ARM64 CI job failed')
            time.sleep(5)
        pods = json.loads(kube(['get', 'pods', '-l', 'job-name=' + name, '-o', 'json']))['items']
        if len(pods) != 1: raise ValueError('Expected one real qualification pod')
        verify_job(job, pods[0], owner, config['ciImage'])
        if 'NODEHARBOR_ARM64_CI_OK' not in kube(['logs', 'job/' + name]): raise ValueError('The actual ARM64 computation did not pass')
        phone.instrument(['FleetStopContract'], timeout=180)
        if phone.vpn() != before: raise ValueError('The phone VPN policy changed during the real worker test')
        evidence = dict(version=version, commit=commit, apkSha256=checksum(apk), certificateSha256=certificate(apk),
                        signedRelease=True, physical=True, apiLevels=sorted({report['apiLevel'] for report in reports}),
                        ownerControlsPassed=True, vpnPreserved=True, ciQualified=True, arm64JobSucceeded=True,
                        upgradePassed=True, upgradeFromVersion=previous)
        if evidence['certificateSha256'] != manifest['certificateSha256']: raise ValueError('The tested APK signing identity changed')
        validate_android_qualification(evidence, version, commit, checksum(apk))
        manifest['qualification'] = evidence
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
        (folder / 'android-qualification.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print('The signed Android APK passed physical, owner-control and real ARM64 CI qualification')
    finally:
        # Instrumentation may fail before returning the identity; stop through the
        # app's normal owner control even in that case. Enrollment remains intact.
        try: phone.instrument(['FleetStopContract'], timeout=180)
        finally:
            if created: kube(['delete', 'job', name, '--wait=true', '--timeout=60s'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('folder', type=Path); parser.add_argument('version'); parser.add_argument('commit')
    args = parser.parse_args()
    path = os.environ.get('NODEHARBOR_ANDROID_QUALIFICATION_CONFIG')
    if not path: raise ValueError('Configure a private qualification file for dedicated Android devices and a real fleet')
    qualify(args.folder, args.version, args.commit, json.loads(Path(path).read_text()))


if __name__ == '__main__': main()
