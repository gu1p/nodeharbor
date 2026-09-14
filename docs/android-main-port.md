# Android parity with local main

Source: local main `a398fa9f0a6cd6ffe21daf51a77a7fc83a635471`, following
Android commit `663afb689dd57af7b638966eb3e0715787d4c047`. The sixteen intervening
commits are merged, including their shared controller, guest and desktop fixes.
Android now exposes workloads, signed application updates and selected storage
pools through native Compose controls. This is a development port; the earlier
lifecycle milestone and release qualification remain incomplete.

## Behavioral and accessible UI contracts

- Your phone shows Running workloads separately from System components. Health
  probes have a readable name and optional technical details. Unknown inventory
  is shown as unavailable; it never means zero jobs. Draining retains system
  components without counting the controller's identified helpers as user work.
- App updates exposes the installed and available version, automatic updates,
  check, install, cancel, progress and accessible errors. Automatic checks run
  after 30 seconds and every six hours while the app is running. Packages must
  use HTTPS, the embedded release verification key, the Android application ID,
  a newer version and the installed application's signing identity.
- Updates stop new assignments through controller maintenance, wait for a fresh
  confirmed empty workload inventory without evicting jobs or expiring a drain
  deadline, and confirm VM termination before invoking Android's installer.
  Preparation is distinct from waiting for jobs. Cancel remains responsive during
  downloads and controller requests. Android presents its required owner approval;
  background completion makes that action available without changing OS settings.
  Pause/Stop and current sharing preferences survive update and cancellation.
- Storage locations lists the supported app-specific internal and external
  volumes, allocation, availability and configured disks. Arbitrary document
  providers are not block-device storage. No broad file-access permission or
  change to host settings is used to obtain storage access.
- Storage changes are reviewed before applying. Move, grow, shrink, add and remove
  preserve owned worker data; shrink/removal require a verified private backup
  and reviewed temporary capacity. Delete all requires explicit confirmation,
  retains enrollment and disables storage. Only owned files may be removed after
  confirmed process death and controller access cleanup.
  Storage maintenance uses the existing bounded owner drain, including immediate
  teardown at its original deadline. Its review explains possible interruption.
  Application updates retain their separate, non-evicting maintenance behavior.
- Missing selected storage makes the whole worker unavailable. Optional recovery
  defaults off on existing installations, waits two minutes, and requires the
  owner's explicit acceptance of whole-pool data loss. Pause/Stop take precedence.
  Returned stale members stay excluded until reviewed restoration. Journals,
  retained copies and unfinished backup cleanup remain visible after restart.
- Storage changes advance a persisted generation sent in heartbeats. Old health
  evidence cannot qualify the replacement pool. Readiness verifies all selected
  members and guest capacity before admission.
- Shared controller fixes retain Kubernetes PATCH media types, separate RTT from
  DNS work, and apply the shared CI latency percentile policy. Guest updates apply
  the supported NetBird interface exclusion and Kubernetes readiness settings on
  existing owned disks. Guest owner leases remain renewable during startup and
  storage work, without weakening the bounded emergency shutdown coordinator.

## Verification

Testing used the connected stock, verified-boot ARM64 phone running Android API
33. No emulator was started, and existing VPN policy was preserved. The existing
2,560 MiB available-memory preflight remains enforced: 2,048 MiB for the guest
plus a 512 MiB host reserve. Cached background apps were closed within the owner's
earlier authorization; foreground apps, the VPN and system settings were preserved.

Checks on the final port implementation passed:

- `make check`: 149 Python tests (three intentionally skipped), 65 UI tests,
  255 Rust tests (13 native/environment acceptance tests ignored), formatting
  and Clippy. Ignored tests are not platform qualification.
- Android debug and release: 74 unit tests each, lint and APK assembly. The
  release APK is unsigned; the development APK uses the local development key.
- Physical phone: 57 component and accessible UI contracts in 46.389 seconds,
  covering owner controls, background service, isolated descriptors, Binder
  lifetime, network broker, storage journals/copying, update boundaries, seed
  upgrades and the new panels. VPN fingerprints matched before and after.
- Real Ubuntu VM: two-disk pool creation, verified growth, marker persistence,
  retained original copy, unchanged root ownership receipt, and two graceful
  shutdowns with guest poweroff plus Android process death.

| Physical VM cycle | Boot | Guest pool operation | Graceful shutdown | Result |
| --- | ---: | ---: | ---: | --- |
| Create 5 + 10 GiB members | 478.007 s | 130.192 s | 66.675 s | Passed |
| Grow first member to 6 GiB | 328.019 s | 221.874 s | 79.695 s | Passed; flushed marker preserved |

Both boots met 20 minutes; both shutdowns met 120 seconds without forced fallback.
The unenrolled fixture used production VM isolation, owned files and guest storage
helpers. Its fixed test marker cannot enable arbitrary production guest commands.
After confirmed death, only this fixture's disks were removed. No VM remained.

The runtime candidate was frozen before final supervisor cancellation, bounded
storage drain and recovery-selection fixes. It must not be represented as a full
runtime qualification of the final APK. Its identity is:

- Version: `0.1.0-main-port-development`; source base `663afb6`, merged main
  `a398fa9`; working tree modified.
- Source-file manifest SHA-256:
  `91bb304631d749d973b413255b9f4f9afdfd3f986e761ead6753d4aceb6effde`.
- APK SHA-256:
  `0fbe1f5231ca47778ddf8f60d84488df1481ba8eeb3d422921fba9054b7fbdfb`.
- Test APK SHA-256:
  `73be6a62f01300d4b4c690f993ab60d260426c9adaaa409c5c45240c8c0a890d`.

The final development APK and sanitized evidence are local artifacts under
`dist/nodeharbor-android-main-port-development*`. Evidence records its exact
source commit, working-tree status, APK digest, build checks and physical component
results separately from the frozen runtime candidate. Private logs and device
identities remain outside version control. Nothing was published.

### Recorded failures before fixes

Behavioral and accessible UI contracts preceded implementation. Private red-check
records are retained under `.local/android-main-port-*`; raw instrumentation logs
remain under `.local/android-device/`.

- The first merge check failed because the signed update publisher did not
  recognize Android in the combined target list. Android now has its APK target.
- UI contracts initially failed for the three missing panels. Unit and native
  contracts then failed for update parsing/verification, installer permissions,
  storage review/journaling, owned descriptor capabilities and guest storage work.
- Recovery checks exposed lost recovery intent on explicit deletion and unsafe
  selection of replacement capacity. Explicit deletion now cancels recovery;
  automatic recovery retains only remaining selected allocations and requires
  the original minimum pool size and free-space reserve. Returned stale disks
  require a review allocating fresh members.
- Final regressions reproduced an accepted storage callback after service
  destruction and absent bounded-drain UI disclosure. Both physical tests failed
  before the fix; the new drain decision unit test also failed before implementation.
  All now pass, and service destruction retains the journal hold.
- Two test-harness errors were corrected: JSON seed paths were compared before
  decoding escapes, and a dialog button was incorrectly treated as a scroll
  container. These were test errors rather than runtime successes.

One unrelated inherited desktop test,
`an_earlier_worker_error_does_not_reject_a_new_update_before_inspection`, timed out
once in `crates/agent/tests/recreation.rs`. Its focused rerun and subsequent full
checks passed. One check invocation also stopped because the execution sandbox
could not write the existing external tool cache; rerunning with approved cache
access passed without changing cache permissions or host configuration.

### Implementation boundaries and remaining acceptance

Guest control revision **6** refreshes the owned seed atomically and reruns guest
configuration on existing disks without replacing enrollment or worker data.
Readiness requires the expected control/configuration revision and storage
generation/pool identity. Storage commands remain bounded and asynchronous so
owner leases and emergency teardown continue during pool work.

Storage uses up to sixteen files in Android app-specific platform volumes, with
filesystem-aware temporary capacity accounting. The private system disk remains
separate from the contributed pool. Original disks and verified whole-pool backups
are retained until explicit cleanup. Automatic data-loss recovery defaults off,
waits two minutes, respects Pause/Stop, and never expands selected allocations or
silently uses another volume. Its controller reset identity survives interruption.

The updater checks HTTPS metadata and authenticates streamed APK bytes with the
embedded release key, then verifies package identity, newer version, source commit
metadata and the installed signing certificate before PackageInstaller. Android
requires the owner's installation permission and approval. Clean numeric release
builds can check automatically; development builds require an explicit check.

The following are **outstanding**, not passing acceptance claims:

- A complete signed-release installation through Android's approval UI; release
  signing credentials and a matching newer release were not provided.
- Fleet-enrolled workload drain/reset/requalification against a real controller.
  The physical VM fixture was deliberately unenrolled and used no fleet credentials.
- Real removable-volume disconnect/reconnect and automatic whole-pool recovery;
  the connected phone test used two owned files on its internal filesystem.
- Physical full-VM shrink/removal/restore acceptance; backup integrity, cancellation,
  journal recovery and cleanup were exercised by unit/component tests.
- Five consecutive graceful lifecycle cycles on the final APK, the complete final
  fault matrix, one hour of screen-off operation, and API 36/37 acceptance. Earlier
  lifecycle evidence does not substitute for these checks on this port. Emulators
  were excluded at the owner's request because of host memory limits.

To repeat the real pool fixture, build the debug and Android test APKs and run
`PoolVmContract#twoDisksPreserveDataAcrossVerifiedGrowthAndGracefulRestart` on a
dedicated stock ARM64 phone with at least 2.5 GiB available RAM and 50 GiB free
storage. It downloads verified pinned Ubuntu inputs by default; the optional
`verifiedDownloads=true` argument reuses locally cached inputs that are verified
again. Never install another APK or run concurrent instrumentation during the VM
fixture; preserve disks if process death cannot be confirmed.

Android platform references: [app-specific storage](https://developer.android.com/training/data-storage/app-specific),
[installer user action](https://developer.android.com/reference/android/content/pm/PackageInstaller.SessionParams#setRequireUserAction(int)),
and [pending installer approval](https://developer.android.com/reference/android/content/pm/PackageInstaller#STATUS_PENDING_USER_ACTION).
