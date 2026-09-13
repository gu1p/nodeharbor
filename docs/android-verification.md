# Android development verification

Recorded on 2026-09-13. These results apply to development artifacts in this
working tree, based on `a21633c265a54178f2419d3cdd2b580af761dfd5` with local
changes. That base commit is **not** the tested release source identity.
Publication requires a clean committed build and fresh evidence for its exact APK.

## Passing checks

- `make check`: Python release/guest contracts, 24 accessible web contracts,
  TypeScript/build checks, shared Rust/controller/agent tests, formatting and Clippy.
- Android debug and release variants: 41 unit tests each and lint with warnings
  treated as errors. The release signing path has been exercised with a temporary
  development identity; it is not a production-signed release.
- Stock physical ARM64 Android 13: 29 contracts passed for APK SHA-256
  `6fb18176608c40856897703c1e5504fed7e571d2b6088d28f0f35d6d06b47303`.
  They exercise accessible owner controls, encrypted storage, foreground service
  lifetime and notification Pause, wake ownership, isolated-process restrictions,
  fresh process instances, Linux boot, real broker networking/DNS, bounded private
  frames, owned storage, guest seed ordering and the source/license screen.
  The Android VPN fingerprint was unchanged before and after testing.
- Earlier physical full-Ubuntu probes demonstrated boot and real HTTPS, including
  a 565.859-second successful run. Other attempts timed out; these probes do not
  establish sustained CI contribution or production guest preparation.
- A fresh pinned QEMU/GLib/libslirp build, Android ARM64 ELF validation and 16 KiB
  segment alignment passed. Native binary hashes are recorded in the generated
  runtime receipt; corresponding upstream sources and patches were bundled.

## Failures recorded before corrections

The ignored `.local/` logs retain full failure output. Representative regression
records include:

| Contract | Recorded failure | Correction |
| --- | --- | --- |
| Background owner controls | Missing service/lifecycle and accessible controls | Foreground owner service, independent supervisor and notification actions |
| Fresh isolated sessions | Reusing a terminating isolated process | Public `bindIsolatedService` with a fresh session instance |
| Invalid resource input | Save could retain an old limit; field error was out of view | Validation blocks Save and exposes an adjacent summary |
| Early guest owner channel | Missing readiness notification and late network-dependent startup | Early systemd notify service; watchdog waits for channel readiness |
| Disk preparation budget | Missing accounting for allocated staging blocks | Count actual owned blocks during preparation and after rename |
| Thermal/memory emergency | Missing immediate-stop decision contract | No workload-drain delay at severe heat or critical memory pressure |
| Inactive worker status | Missing inactive-display contract | Refresh cannot report a stopped worker ready |
| Source offer | Source/license control absent from the UI | Native dialog with build identity and source link |
| APK signature verifier | Build-tools 37 emits a scheme-prefixed signer label | Accept official formats while requiring one consistent certificate |
| Guest status during slow startup | The real VM returned a failed first owner command | Unconfigured status avoids systemd; unavailable service status denies readiness without preventing owner leases |

The existing desktop drain test also intermittently failed while waiting three
seconds for its local fixture to initialize under concurrent native builds.
Its fixture allowance is now fifteen seconds. The actual owner-action assertion
still requires shutdown within two seconds; the repository suite then passed.

## Remaining acceptance

- Android 16 emulator: the initial suite passed 24 of 26 contracts; normal DNS
  resolution and guest DNS failed. This host configuration is not qualified.
- Android 17 emulator: 27 of 29 contracts passed on the same APK listed above;
  only normal and guest DNS failed. Espresso's removed-input-API failure was
  corrected with the supported 3.7.0 update. An earlier notification Pause check
  failed under test load; both its isolated rerun and the full rerun passed.
  The emulator VPN fingerprint remained unchanged.
  The same DNS failures persisted with the app's local-network permission granted.
- The large production-path guest test could not start on the physical phone
  because available RAM was below the required VM plus phone reserve. Its
  15 GiB allocation and real Ubuntu boot completed in the emulator, but the first
  owner status command failed during slow startup (1,389.282 seconds including
  preparation). After correction, both boots answered status and lease requests,
  and the first graceful poweroff was confirmed. The second graceful poweroff
  exceeded its 120-second bound, so the full 1,126.949-second contract **failed**.
  The owner's forced isolated-process teardown then succeeded and the temporary
  disk was removed. A complete graceful lifecycle run remains outstanding.
  Cached inputs do not count as networking qualification.
- Real NetBird/K3s enrollment, normal fleet CI qualification, a successful actual
  ARM64 CI job, sustained service qualification, production signing and the
  GitHub release workflow have not passed end to end.
- Android 14/15, other manufacturers and sustained screen-off operation are not
  established by these local results. No host or phone VPN configuration was
  changed to make a test pass.

The release workflow enforces the outstanding signed-APK, physical-device,
API-coverage, owner-control and real-job evidence before publishing an Android
asset. No credentials, device identities or private fleet configuration are
included in this record.
