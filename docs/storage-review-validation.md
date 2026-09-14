# Storage review repairs: validation record

This change repairs the six storage review findings in the existing worktree.
The required native gate is macOS arm64/Intel with Lima/VZ and Linux arm64/x86_64
with Lima/QEMU/KVM. The selected-directory adapter remains tied to pinned Lima.
Passing portable tests does not qualify an untested native target.

## Failures recorded before implementation

Behavioral and accessible UI contracts were added first, provisioning and lease
units second, and Lima protocol integration tests third. Implementation followed.

| Check | Observed failure |
| --- | --- |
| `make check`, new `StorageRecovery.test.tsx` | Four failing UI cases: deletion disabled for the only missing disk; unrelated CPU edits lost after shrink/growth apply; an older poll undid completed deletion. |
| `cargo test --locked -p nodeharbor-controller --test worker_control storage_drain_persists` | Cluster drain observed `(sharing, permitted=true, eligible_ci=true, eligible_services=true)` instead of durable non-acceptance. |
| Agent provisioning/lease unit contracts | Explicit pool identity, reusable provisioning update, and scoped host-work lease APIs were absent (compilation failures recorded before adding them). |
| `cargo test --locked -p nodeharbor-agent --test lima_storage_recovery` | Retained-VM boot still required the original 30 GiB; failure recovery and configuration after Delete all failed with a different pool identity; controller failure still allowed a boot; an expired deadline stopped the VM before publishing non-acceptance. |
| Follow-up controller-outage regressions | A running growth operation stayed running after its heartbeat failed; returned storage issued `start` and `validate` before a failed bootstrap stopped it again. Both failures were recorded before correcting coordination. |

The initial default `make check` wrapper could not write its external cache.
Checks subsequently used installed npm/cargo and a task-local target directory.
The sandbox also blocked local test listeners; isolated controller integration
tests were rerun with approved localhost access. Neither environment failure was
counted as a behavioral regression or a passing check. Fixture corrections for
versioned settings and private backup permissions preceded the behavioral results.

Each implementation change invoked `make check`. During staged repairs, known
failing contracts in later steps remained recorded failures.

## Repairs and ticket coverage

- `/device/drain` commits non-accepting policy and device state before cluster
  work. Storage maintenance publishes non-acceptance each tick, honors its drain
  deadline, and stops or prevents boot when controller coordination fails.
- Retained Lima VMs receive their replacement request, attachments and readiness
  probe through `limactl edit` before boot. Repeating the update is safe; unrelated
  provisioning and ownership data remain intact.
- First boot and later guest requests use the same persisted pool UUID, including
  failure recovery and configuration after Delete all.
- Backup streaming and host verification share scoped owner-lease renewal. A lost
  lease prevents phase advancement; cancellation does not leave a renewal task.
- Missing storage keeps the explicit, confirmed Delete all review available while
  blocking conflicting edits. Active maintenance still blocks competing deletion.
- Snapshot updates merge the storage allowance into the policy draft, preserve
  unrelated edits, reject older storage revisions and retain disabled sharing
  after deletion. Legacy editable disk allowances remain editable.

| Ticket requirement | Retained contract and validation |
| --- | --- |
| Discover labels, locations, free and configured capacity | Inventory and accessible volume controls; native two-volume discovery and primary allocation checks passed. |
| Per-location directory and allocation; resolved default | Existing picker, preview/apply and allocation/default-location tests pass. |
| Multiple volumes with usable combined capacity; add later | Pinned Lima directory adapter and guest LVM pool retained; native disk placement/growth and two-volume pool replacement passed. |
| Space, permissions, overlap, filesystem and runtime validation | Existing planner/runtime/ownership tests pass; native ownership-disabled APFS rejection passed without changing settings. |
| Durable choices; unavailable volumes never redirected | Settings, receipts, volume identities and operation journals retained; disappearance, retry, wrong-identity and interrupted-boot regressions pass. Native host reboot/unplug qualification remains pending. |
| Existing files, owner controls, isolation and restart disclosure | Controller/agent interruption and UI contracts pass; native replacement preserved file data, numeric ownership, mode, xattr, hardlink, symlink and sparse data across VM restart. |
| Opted-in remote configuration inventory and configuration | Existing shared snapshot/preview/apply contracts retained. No new transport or host paths in controller heartbeats. |

## Verified on 2026-09-13

`make check` passed: Python reported 114 passed and three Linux-only installer
checks skipped on macOS; all 65 UI tests, TypeScript/build, workspace Rust tests,
formatting and Clippy with warnings denied passed. Native tests stay explicitly
opt-in. Results apply to this uncommitted worktree; no release package was produced.

The standalone `native_lima_replacement` test passed on macOS arm64 with pinned
Lima 2.2.0/VZ and two disposable, ownership-enabled APFS volumes. It created a
30 GiB pool, enabled the real guest watchdog, deliberately spent 150 seconds in
host verification with lease renewal, backed up and retired the pool, removed its
source images, refreshed the retained VM's provisioning twice, restored onto a
fresh 15 GiB pool, and verified capacity, identities, data and metadata after
restart. It used its own unenrolled VM. This does not prove controller/Kubernetes
qualification or native destructive recovery after permanent loss.

Native inventory, primary allocation, disk placement/growth across a new runner,
and read-only ownership rejection also passed. Initially, the disposable volumes
were mounted with `nobrowse`; discovery rejected them because sysinfo omits such
mounts. Remounting only the test volumes as ordinary browsable mounts made those
checks pass. Non-browsable volumes remain an unsupported selection and are never
redirected. All fixture VMs, images and mount directories were cleaned up.

| Host/runtime | Result for these repairs |
| --- | --- |
| macOS arm64, browsable ownership-enabled APFS, Lima/VZ | Full portable suite and the native checks above passed. Enrolled lifecycle, host reboot and unplug/failure matrix still pending. |
| macOS Intel, APFS, Lima/VZ | Agent and tests compile for `x86_64-apple-darwin`; native execution and desktop/package validation pending. |
| Linux x86_64, local ext4/XFS, Lima/QEMU/KVM | Agent and tests compile for `x86_64-unknown-linux-gnu` using Zig; native KVM, desktop/package and multi-volume validation pending. |
| Linux arm64, local ext4/XFS, Lima/QEMU/KVM | Agent and tests compile for `aarch64-unknown-linux-gnu` using Zig; native KVM, desktop/package and multi-volume validation pending. |
| Windows/Multipass | No new support or validation claim; selectable multi-disk placement remains unsupported. |

Cross-checks used `cargo check --locked -p nodeharbor-agent --tests --target
x86_64-apple-darwin` and `cargo-zigbuild check --locked -p nodeharbor-agent --tests
--target <linux-target>`. The initial Linux attempt hit an incompatible local
compiler-cache wrapper; disabling it for the command allowed compilation to pass.
Compilation does not count as native acceptance.

The all-four native gate remains open: isolated Intel macOS and both Linux host
architectures, plus enrolled lifecycle fixtures, were not supplied. Run the
standalone and enrolled native tests on each supported target, including the
failure/restart matrix in [the lifecycle validation notes](storage-lifecycle-validation.md).
Requalify the pinned directory adapter whenever Lima changes. Other rejected
combinations remain ownership-disabled APFS, unsupported/network filesystems,
Linux without accessible KVM/QEMU, and cross-architecture workers.
