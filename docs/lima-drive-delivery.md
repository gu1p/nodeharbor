# Lima, drive selection, and release validation

Linux configuration creation now selects Lima in the store used by the desktop
and CLI. Opening obsolete Multipass settings privately archives the original
bytes and creates a new unenrolled identity with sharing off. An active
supervisor prevents reset. Existing VM data is not migrated. Linux execution and
streaming through Multipass are rejected before a process starts. Windows keeps
Multipass. Host networking, VPN, authentication, owner receipts, and health
qualification remain required.

The storage picker selects mounted filesystems and individual GiB allocations.
Inventory includes the OS-reported drive type and a suggested folder. Existing
application-volume storage uses its managed directory; other volumes suggest a
NodeHarbor subdirectory. The owner can customize folders. Picker requests include
an expected volume identity; review and application revalidate it. Existing
filesystem eligibility, write probes, overlap checks, pool capacity accounting,
system-disk reserve, Lima disk receipts, and guest-pool verification remain active.

The simulated macOS permission test now lives in the library's unit-test build.
Its preflight seam is private and absent from production builds; public Lima
constructors retain native preflight.

Release reconciliation considers all ancestry of main and selects the highest
valid numeric published release. Tests include v0.1.57 versus merged v0.1.72,
invalid versions, unpublished releases, and unrelated commits. The update
channel retains its existing version comparison and signature checks.

Android release builds additionally produce a lower-version qualification APK
from the same clean source. Both versions and both instrumentation APKs must
share one certificate. Qualification installs the lower version, seeds private
settings, an encrypted token fixture, and a file, then updates in place and
verifies the installed version and persisted data. VPN fingerprints must remain
unchanged. The unified release gate requires this evidence in addition to the
existing API 33/36/37, physical-phone, owner-control, and real fleet-job checks.

## Recorded checks

Failing checks were recorded before fixes for release ordering/ancestry, Linux
store setup/archive behavior, picker accessibility and identities, the private
permission-test seam, and signed Android upgrade evidence. Local diagnostic logs
are retained under `.local/lima-drive-delivery/` and are not release evidence.

The local macOS ARM64 `make check`, keyboard-only Chromium picker contract, and
Android debug unit tests, lint, app build, and instrumentation build passed.
The Android runtime was built from its pinned sources and verified. Persistent
Android signing material is kept outside the repository; the four existing CI
signing secrets have been populated.

## Acceptance still required

Native desktop matrix results, the exact packaged Linux worker on two supported
volumes, persistence and missing-drive behavior, signed Android device updates,
API 36/37 coverage, and trusted-main fleet qualification remain required. No new
release has been published by this change. Native qualification must identify the
exact tested source commit and version before release delivery can be complete.
