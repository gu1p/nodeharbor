#!/usr/bin/env python3
"""Run APK contracts on explicitly selected Android devices, retaining private logs."""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile
import uuid
from release import checksum

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = 'io.github.gu1p.nodeharbor'
BASIC_CLASSES = ['LaunchContract', 'NativeRulesContract', 'PrivateStoreContract', 'SandboxContract',
                 'SandboxLifetimeContract', 'VmBootContract', 'NetworkBrokerContract', 'GuestNetworkContract',
                 'GuestChannelContract', 'GuestStorageContract', 'GuestSeedContract', 'VmSessionContract', 'WakeLockContract',
                 'BackgroundContract#backgroundExecutionUsesAnUnexportedForegroundServiceThatSurvivesClosingTheTask']


def instrumentation_values(output):
    if not re.search(r'OK \([1-9][0-9]* tests?\)', output) or 'FAILURES!!!' in output or 'INSTRUMENTATION_FAILED' in output:
        raise ValueError('The Android contracts did not pass')
    # AndroidJUnitRunner may emit its class prefix without a newline before a
    # status Bundle. Each Bundle entry itself still has a fixed platform prefix.
    return dict(re.findall(r'INSTRUMENTATION_STATUS: ([A-Za-z][A-Za-z0-9]*)=([^\r\n]*)', output))


class AndroidDevice:
    def __init__(self, serial=None):
        self.adb = os.environ.get('ADB') or shutil.which('adb')
        if not self.adb: raise ValueError('Install Android platform-tools and authorize the test device')
        listing = subprocess.check_output([self.adb, 'devices'], text=True)
        ready = [line.split()[0] for line in listing.splitlines()[1:] if line.endswith('\tdevice')]
        if serial is None:
            if len(ready) != 1: raise ValueError('Select one authorized dedicated Android test device')
            serial = ready[0]
        if serial not in ready: raise ValueError('The selected Android test device is not authorized or connected')
        self.serial = serial
        self.api = int(self.command(['shell', 'getprop', 'ro.build.version.sdk']).strip())
        self.physical = self.command(['shell', 'getprop', 'ro.kernel.qemu']).strip() != '1'
        if 'arm64-v8a' not in self.command(['shell', 'getprop', 'ro.product.cpu.abilist']):
            raise ValueError('Android worker qualification requires an ARM64 device')
        if self.physical and (self.command(['shell', 'getprop', 'ro.boot.verifiedbootstate']).strip() != 'green' or
                              self.command(['shell', 'getprop', 'ro.build.type']).strip() != 'user'):
            raise ValueError('Qualify a supported stock phone with verified boot; its settings have been preserved')

    def command(self, args, timeout=60, input=None):
        try:
            result = subprocess.run([self.adb, '-s', self.serial, *args], input=input,
                                    capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            raise RuntimeError('The Android operation exceeded its deadline') from None
        if result.returncode: raise RuntimeError('The Android operation failed; the device settings were preserved')
        return result.stdout

    def install(self, apk, tests):
        for file in [apk, tests]:
            if 'Success' not in self.command(['install', '-r', '-t', str(file)], timeout=300):
                raise ValueError('APK installation failed. Existing app data and signing identities were preserved.')

    def installed_version(self):
        listing = self.command(['shell', 'dumpsys', 'package', PACKAGE])
        match = re.search(r'\bversionCode=(\d+)\b', listing)
        return int(match[1]) if match else None

    def reset_test_installation(self, expected_certificate):
        """Explicitly opted-in reset of this signed test app, after owner shutdown."""
        from build_android import certificate
        paths = [line.removeprefix('package:') for line in self.command(['shell', 'pm', 'path', PACKAGE]).splitlines()
                 if line.startswith('package:')]
        if not paths: return
        if len(paths) != 1: raise ValueError('The disposable installation must be the standalone qualification APK')
        with tempfile.TemporaryDirectory(prefix='nodeharbor-installed-apk-') as directory:
            apk = Path(directory) / 'installed.apk'
            self.command(['pull', paths[0], str(apk)], timeout=120)
            if certificate(apk) != expected_certificate:
                raise ValueError('The installed signing identity differs; its app and data were preserved')
        self.instrument(['FleetStopContract'], timeout=180)
        for package in [PACKAGE + '.test', PACKAGE]:
            if 'Success' not in self.command(['uninstall', package], timeout=120):
                raise ValueError('The disposable test installation could not be removed')

    def instrument(self, classes, arguments=None, timeout=1800):
        names = ','.join(PACKAGE + '.' + name for name in classes)
        args = ['am', 'instrument', '-w', '-e', 'class', names]
        for key, value in (arguments or {}).items(): args += ['-e', key, value]
        args += [PACKAGE + '.test/androidx.test.runner.AndroidJUnitRunner']
        # Provisioning arguments travel on stdin, never the host process command line.
        output = self.command(['shell', 'sh'], input=shlex.join(args) + '\n', timeout=timeout)
        logs = ROOT / '.local/android-device'
        logs.mkdir(parents=True, exist_ok=True)
        log = logs / (str(uuid.uuid4()) + '.log')
        log.write_text(output.replace(self.serial, '[device]')); log.chmod(0o600)
        try: return instrumentation_values(output)
        except ValueError: raise ValueError('Android contracts failed. Private diagnostics: ' + str(log)) from None

    def vpn(self):
        value = self.instrument(['VpnSnapshotContract'], timeout=60)
        if not re.fullmatch('[0-9a-f]{64}', value.get('vpnFingerprint', '')):
            raise ValueError('Android could not record the existing VPN policy')
        return value['vpnFingerprint']


def basic(device, apk, tests, debug_ui=False):
    digest = checksum(apk)  # Capture the actual installed file before any other build can replace it.
    device.install(apk, tests)
    before = device.vpn()
    device.instrument(BASIC_CLASSES + (['OwnerUiContract', 'BackgroundContract#closingTheActivityKeepsTheOptedInServiceAndNotificationPauseStopsIt'] if debug_ui else []), timeout=300)
    if device.vpn() != before: raise ValueError('The device VPN policy changed during qualification')
    return dict(apkSha256=digest, apiLevel=device.api, physical=device.physical,
                ownerControlsPassed=True, vpnPreserved=True, passed=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apk', type=Path); parser.add_argument('tests', type=Path)
    parser.add_argument('--serial'); parser.add_argument('--debug-ui', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = basic(AndroidDevice(args.serial), args.apk, args.tests, args.debug_ui)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print('Android API ' + str(result['apiLevel']) + ' device contracts passed')


if __name__ == '__main__': main()
