#!/usr/bin/env python3
"""Build an identified ARM64 APK, its instrumentation package and runtime sources."""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
from release import android_version_code, checksum

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'aarch64-linux-android'


def verify_badging(text, version):
    expected = f"package: name='io.github.gu1p.nodeharbor' versionCode='{android_version_code(version)}' versionName='{version}'"
    if expected not in text or re.search(r"^native-code: 'arm64-v8a'\s*$", text, re.MULTILINE) is None:
        raise ValueError('The APK does not match its requested package, version or ARM64-only architecture')


def sdk_tool(name):
    sdk = Path(os.environ.get('ANDROID_HOME') or os.environ.get('ANDROID_SDK_ROOT', ''))
    tool = sdk / 'build-tools/37.0.0' / name
    if not tool.is_file(): raise ValueError('Install Android build-tools 37.0.0 and set ANDROID_HOME')
    return str(tool)


def certificate_digest(output):
    identities = set(re.findall(r'^(?:Signer #\d+|V\d(?:\.\d+)? Signer(?: #\d+)?):? certificate SHA-256 digest: ([0-9a-f]{64})$',
                                output, re.MULTILINE))
    if re.search(r'^Number of signers: 1$', output, re.MULTILINE) is None or len(identities) != 1:
        raise ValueError('The APK must have one verifiable signing identity')
    return identities.pop()


def certificate(apk):
    result = subprocess.run([sdk_tool('apksigner'), 'verify', '--verbose', '--print-certs', str(apk)],
                            check=True, capture_output=True, text=True)
    return certificate_digest(result.stdout)


def build(version, commit, release, allow_dirty=False):
    android_version_code(version)
    if not re.fullmatch('[0-9a-f]{40}', commit): raise ValueError('Use the full source commit')
    actual = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True))
    if actual != commit or (dirty and (release or not allow_dirty)):
        raise ValueError('Release APKs require the exact clean source commit; use --allow-dirty only for development builds')
    env = dict(os.environ, NODEHARBOR_VERSION=version, NODEHARBOR_COMMIT=commit,
               NODEHARBOR_ANDROID_RELEASE='1' if release else '0', NODEHARBOR_DIRTY_SOURCE=str(dirty).lower())
    variant = 'Release' if release else 'Debug'
    wrapper = ROOT / 'android/gradlew'
    subprocess.run([os.sys.executable, str(ROOT / 'scripts/android_probe.py')], cwd=ROOT, env=env, check=True)
    with tempfile.TemporaryDirectory(prefix='nodeharbor-android-signing-') as directory:
        if release and env.get('NODEHARBOR_ANDROID_KEYSTORE_BASE64'):
            key = Path(directory) / 'distribution.p12'
            key.write_bytes(base64.b64decode(env.pop('NODEHARBOR_ANDROID_KEYSTORE_BASE64'), validate=True))
            key.chmod(0o600)
            env['NODEHARBOR_ANDROID_KEYSTORE'] = str(key)
        subprocess.run([str(wrapper), f':app:test{variant}UnitTest', f':app:lint{variant}',
                        f':app:assemble{variant}', f':app:assemble{variant}AndroidTest'], cwd=ROOT / 'android', env=env, check=True)
    apk = ROOT / f'android/app/build/outputs/apk/{variant.lower()}/app-{variant.lower()}.apk'
    test = ROOT / f'android/app/build/outputs/apk/androidTest/{variant.lower()}/app-{variant.lower()}-androidTest.apk'
    badging = subprocess.check_output([sdk_tool('aapt2'), 'dump', 'badging', str(apk)], text=True)
    verify_badging(badging, version)
    cert = certificate(apk)
    if certificate(test) != cert: raise ValueError('Instrumentation must be signed by the tested app’s signing key')
    output = ROOT / 'dist'
    output.mkdir(exist_ok=True)
    prefix = f'nodeharbor-v{version}-{TARGET}'
    destination = output / (prefix + '.apk')
    shutil.copyfile(apk, destination)
    shutil.copyfile(test, output / 'android-tests.apk')
    runtime_sources = ROOT / '.local/android-native/nodeharbor-android-runtime-source.tar.gz'
    if not runtime_sources.is_file(): raise ValueError('Build the corresponding runtime sources with --bundle-sources first')
    sources = output / (prefix + '-runtime-source.tar.gz')
    with tempfile.TemporaryDirectory(prefix='nodeharbor-android-source-') as directory:
        project = Path(directory) / 'nodeharbor-source.tar'
        subprocess.run(['git', 'archive', '--format=tar', '--prefix=nodeharbor/', '-o', str(project), commit], cwd=ROOT, check=True)
        with tarfile.open(sources, 'w:gz') as archive:
            archive.add(runtime_sources, arcname='runtime-source.tar.gz')
            archive.add(project, arcname='nodeharbor-source.tar')
    manifest = dict(version=version, commit=commit, target=TARGET, os='android', arch='arm64',
                    signedRelease=release, dirtySource=dirty, certificateSha256=cert,
                    assets=[dict(name=p.name, sha256=checksum(p)) for p in [destination, sources]])
    (output / (prefix + '.json')).write_text(json.dumps(manifest, indent=2) + '\n')
    print('Built and verified ' + destination.name + '; native device and acceptance qualification are still required')


def previous_version(version):
    code = android_version_code(version) - 1
    if code < 1: raise ValueError('Signed upgrade qualification needs a lower installable version')
    return f'{code // 100_000_000}.{code % 100_000_000 // 1_000_000}.{code % 1_000_000}'


def build_upgrade_pair(version, commit):
    previous = previous_version(version)
    build(previous, commit, True)
    output = ROOT / 'dist'
    baseline = output / 'android-upgrade'
    baseline.mkdir(exist_ok=True)
    for path in [*output.glob(f'nodeharbor-v{previous}-{TARGET}*'), output / 'android-tests.apk']:
        shutil.move(str(path), baseline / path.name)
    build(version, commit, True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('version'); parser.add_argument('commit')
    parser.add_argument('--release', action='store_true')
    parser.add_argument('--allow-dirty', action='store_true')
    parser.add_argument('--upgrade-pair', action='store_true')
    args = parser.parse_args()
    if args.upgrade_pair:
        if not args.release or args.allow_dirty: raise ValueError('Upgrade qualification requires signed clean release builds')
        build_upgrade_pair(args.version, args.commit)
    else:
        build(args.version, args.commit, args.release, args.allow_dirty)


if __name__ == '__main__': main()
