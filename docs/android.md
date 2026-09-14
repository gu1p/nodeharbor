# Android worker

The native Kotlin/Compose app shares NodeHarbor's dark surfaces, mint accent,
navigation, enrollment, resource controls and qualification model. Its ARM64
runtime executes an Ubuntu VM with QEMU TCG; Kubernetes containers run inside
that guest. It does not require root, a privileged system-app installation or
access to the phone's hypervisor.

## Installation and continuous operation

The release pipeline produces
`nodeharbor-v<VERSION>-aarch64-linux-android.apk` and publishes it beside the
desktop installers only after Android qualification passes. Android distribution
is currently awaiting the verification and provisioning listed below. Local
development APKs are not qualified releases.

Install the APK using Android's package installer, connect with your fleet's
HTTPS address and enrollment code, review Sharing rules, and prepare the worker.
Installation and enrollment leave sharing off. The initial download is about
600 MiB; allow at least 26 GiB free for the default 15 GiB worker and preparation
overhead. A 2 GiB VM requires at least 2.5 GiB available RAM before startup.

Choose **Continuous sharing setup** to enable the background, open, boot-after-
unlock and screen-off wake preferences, then review each setting. Battery and
metered-network use remain separate opt-ins. Native Android settings expose
notification permission and battery restrictions; Android 17 also requires local
network permission. The app reports the actual grants and restrictions.

An owner-enabled foreground service keeps the VM independent of the activity.
Its ongoing notification provides Pause and Stop. CPU wake locks have bounded
leases and are renewed only while the owner permits execution. Force-stop,
revocation, stale controller authority, severe heat and memory pressure can stop
work. Android and manufacturer resource policies still apply: the app cannot
promise that the operating system will keep a process alive indefinitely.
Updates preserve enrollment and policy, and pause sharing.
Android's [foreground-service documentation](https://developer.android.com/develop/background-work/services/fgs)
describes the notification and platform-lifetime requirements used here.

## Isolation and networking

QEMU and libslirp run in a fresh Android isolated process for each VM session.
They receive owned file descriptors and a bounded packet broker, never enrollment
credentials, arbitrary file paths, Android network permission, a host shell or
device access. The broker uses ordinary Android sockets and the current Android
DNS resolver. It preserves the active VPN and fails if that policy does not allow
the required connectivity. There is no Android `VpnService` or routing exception.
The managed guest runs its own NetBird client inside the VM.

The encrypted enrollment credential is bound to its controller and device.
Storage receipts identify the owned VM. A bounded private virtio channel accepts
only owner-scoped status, configuration, lease renewal and poweroff commands.
Preparation verifies every Ubuntu download against pinned SHA-256 values.
Replacement requires confirmed isolated-process shutdown and fleet access removal
before deleting owned files. Unknown files and ownership mismatches are preserved.

The controller uses the same protected qualification labels and health windows as
desktop workers. Android enrollment does not skip network checks, CI qualification,
service observation, quarantine or Kubernetes scheduling policy.

## Build and test

Use macOS or x86_64 Linux, Python 3.11+, JDK 21, Rust 1.98.0 with
`aarch64-linux-android`, `pkg-config`, Meson 1.9.1 and Ninja 1.13.0. Install Android
platform 37, build-tools 37.0.0, NDK 28.2.13676358 and CMake 3.22.1; set
`ANDROID_HOME` to that SDK. The Gradle wrapper pins and verifies Gradle 9.3.1.
Upstream runtime and Ubuntu inputs are pinned in `android/runtime/lock.json`.
All packaged native libraries are also checked for ARM64 and 16 KiB alignment;
see Android's [page-size guidance](https://developer.android.com/guide/practices/page-sizes).

```sh
python3 scripts/build_android_runtime.py --bundle-sources
python3 scripts/android_probe.py
make check
cd android
./gradlew :app:testDebugUnitTest :app:lintDebug :app:assembleDebug :app:assembleDebugAndroidTest
```

Run the packaged-runtime and accessible UI contracts on one explicitly authorized
ARM64 test device:

```sh
python3 scripts/test_android.py \
  android/app/build/outputs/apk/debug/app-debug.apk \
  android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk \
  --debug-ui --output .local/android-device-report.json
```

Use `--serial` when multiple devices are connected. Diagnostics remain in ignored,
private local files. `make check` covers shared Rust, controller, Python and web
contracts; it does not replace Android Gradle or device checks. The larger
`OwnedVmContract` exercises real preparation, five graceful same-disk cycles with
a flushed guest marker, and separate failure/recovery contracts. It requires the
full disk and memory budget. See `android-lifecycle-verification.md` for current
implementation results and outstanding acceptance. Use only the connected
physical phone for this work; the development host cannot spare emulator memory.

## Release configuration

`scripts/build_android.py VERSION COMMIT --release` requires an exact clean source
commit. It builds unit/lint checks, signs the application and test APK with the
same identity, verifies package/version/ABI/signature, and writes checksums plus
source identity into an internal manifest. Development builds may explicitly use
`--allow-dirty`; publication rejects them.

Configure these GitHub Actions secrets through your private signing-key process:

- `NODEHARBOR_ANDROID_KEYSTORE_BASE64`
- `NODEHARBOR_ANDROID_STORE_PASSWORD`
- `NODEHARBOR_ANDROID_KEY_ALIAS`
- `NODEHARBOR_ANDROID_KEY_PASSWORD`

Keep the production signing key backed up securely: updates must use the same
identity. No key, password, device identity or fleet endpoint belongs in this repo.

The trusted-main qualification job uses a dedicated self-hosted runner labeled
`nodeharbor-android`. Set `NODEHARBOR_ANDROID_DEVICES_FILE` to a private JSON file
with a `devices` array of connected ARM64 API 33, 36 and 37 test devices,
including a stock physical phone. This runner needs ADB, Python and Android
build-tools. Existing application signing identities and VPN policy are preserved.

The default release gate combines signed native device/upgrade checks with the
exact-commit [reproducible controller and agent scenarios](reproducible-acceptance.md).
It requires no existing deployment or infrastructure credentials. Reports clearly
identify simulated infrastructure and do not claim live cluster qualification.
All five desktop targets and controller image checks remain required. Corresponding
QEMU/GLib/libslirp sources, licenses, patches and build scripts are published as
a matching `-runtime-source.tar.gz` asset.

For optional additional testing against an existing deployment, use
`qualify_android.py --external-config PATH`. That private JSON includes
`dedicatedDevices: true`, `devices`, `controller`, a fresh `enrollmentCode`,
`kubernetesContext`, `namespace`, and `ciImage` pinned by SHA-256 with ARM64 Python 3.
This mode additionally needs kubectl and retains the normal qualification window,
actual ARM64 Kubernetes workload and owner-stop checks. It uses the supplied
kubeconfig unchanged.

## Verification limits

Physical Android 13 tests have demonstrated native UI controls, foreground-service
lifecycle, isolated-process restrictions, Linux boot, broker networking and
unchanged VPN state. A full Ubuntu boot with HTTPS also passed. These are
development-build results, not sustained fleet acceptance.

Earlier Android 16/17 emulator runs could not resolve public DNS under the
existing host policy. Later API 36/37 native contract runs passed using the
existing default resolver without changing the host VPN. Android 17's test-framework input API failure was corrected;
its UI and background-service contracts now pass. Neither platform is qualified.
Host VPN and routing settings have been preserved. The physical phone's available
RAM also prevented the larger production preparation test at its last attempt.
An emulator retry completed both private-channel boots, but its second graceful
poweroff exceeded the test deadline; forced owner teardown succeeded. That full
lifecycle contract remains failed.

Current release qualification uses persistent signing, the API 33/36/37 native
device checks and reproducible acceptance reports described above. Historical
development results do not qualify a newer APK. Existing-deployment tests are
optional additional coverage. No Android APK has been published as a qualified
GitHub release. See `android-contracts.md` for the
acceptance contracts and `android-verification.md` for the local check record.
