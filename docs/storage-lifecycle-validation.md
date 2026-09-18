# Storage lifecycle implementation and validation

This extends the pre-existing, uncommitted pooled-storage implementation in this
worktree. No commit, merge, release, or host VPN change was performed. Native
results in `storage-724.md` predate this workflow and do not qualify these changes.

## Behavior implemented

- Shrink allocation and Remove disk review resulting capacity, a conservative
  data-fit estimate, private backup space, downtime, and the replaced images.
  Preview requires the existing worker to be reachable for read-only data inspection;
  a stopped worker must be started by its owner before this review.
- The recorded operation drains assignments, waits for remaining workloads to leave,
  stops Kubernetes/containerd and their mounts, streams and verifies a backup,
  durably records its SHA-256 checksum, and then replaces source storage. The
  backup is streamed back into the owned Linux guest and checked before bootstrap
  and an actual Kubernetes Node capacity check. The archive is never extracted
  onto the host. Replacement uses fresh pool, image, and filesystem identities.
- The archive preserves file contents, sparse holes, numeric ownership, permissions,
  hard links, symbolic links, FIFOs, device entries, and extended attributes.
  Linux POSIX ACLs are represented by their system extended attributes. Workload
  sockets are removed only after stopping their processes inside the guest.
- Each phase, backup verification, missing-volume timer, retired image, configured
  location, active generation, and cleanup record survives application restart.
  An error latches the operation; Retry resumes its saved phase. Pause/Stop cancels
  active work and retains the recovery record. Backup cleanup resumes automatically.
  Unavailable retired images remain listed; after Delete all, reconnect the original
  volume and use Delete all again to finish deleting these images.
- Delete all worker storage revokes worker access through the existing controller
  reset process, deletes owned storage, disables sharing and storage, and retains
  host enrollment. It cannot silently recreate the default 30 GiB allocation.
  Retiring the pool also removes the guest's Harbor Build cache link; a paused
  worker keeps it.
- Missing storage stops assignments and the affected VM immediately. Checks run
  every ten seconds for two minutes. A returned pool must pass complete identity
  and pool validation; there is no forced mount or partial LVM activation.
- Automatic destructive recovery requires persisted owner consent, defaults off
  for existing installations, and respects Pause/Stop. It resets controller access
  before deleting surviving images and rebuilds only on remaining selected locations
  with at least 15 GiB combined allocation and the existing host reserve. Errors
  stop recovery instead of triggering repeated rebuilds. Returned stale disks remain
  excluded until the owner reviews Restore this disk's capacity.
- Snapshots show active and configured capacity separately. Ordinary heartbeats
  contain the active capacity and storage generation, without host paths. A changed
  generation invalidates prior controller health qualification and requires a drain
  before new qualification. Kubernetes retry policies determine workload restart;
  the product does not promise that every interrupted job retries.

## Runtime and capacity boundaries

Lima on macOS/Linux uses replacement additional-disk images; it never attempts
Lima's unsupported in-place shrink. Owned images are detached and deleted using
the supported disk command, without force/unlock, followed by private backing-file
cleanup. The old and new complete disk sets need not be attached together.
[Pinned Lima disk commands](https://github.com/lima-vm/lima/blob/v2.2.0/cmd/limactl/disk.go).

Windows uses Multipass execution streams and replaces the single owned VM;
runtime credentials are regenerated through normal bootstrap. Additional disks
and per-instance location selection remain unavailable. Shrink currently requires
the Hyper-V driver and permission to read its storage metadata. A read-only
`Get-VMHardDiskDrive` query must match the documented daemon storage configuration;
actual allocated file bytes receive reclaim credit. The backup and replacement
peak must leave the host reserve. VirtualBox, inaccessible metadata, or mismatched
daemon storage configuration fail review before drain or deletion. This does not
switch drivers, grant permissions, change daemon configuration, or mount host
folders into the VM. Legacy Multipass shrink on macOS/Linux is unsupported.
[Multipass execution](https://canonical.com/multipass/docs/latest/explanation/multipass-exec-and-shells/),
[daemon storage configuration](https://canonical.com/multipass/docs/latest/how-to-guides/customise-multipass/configure-where-multipass-stores-external-data/),
[Get-VMHardDiskDrive](https://learn.microsoft.com/en-us/powershell/module/hyper-v/get-vmharddiskdrive).

Free-space review is conservative and rechecked when applying. Multipass checks
again after backup, before source deletion. Competing host writes can still consume
space during an operation; failures retain the verified backup and phase. Size
estimates include filesystem/inode overhead, operating space, and the existing
10 GiB host reserve. Backups cannot be placed inside managed directories scheduled
for deletion. No storage location outside the selected set is added during recovery.

## Recorded TDD evidence, 2026-09-13

Behavior contracts were added first, then units, integration contracts, and
implementation. `make check` was run after each logical change. Significant
recorded failures and their fixes:

| Failing check before fix | Resulting correction |
| --- | --- |
| Four accessible shrink/remove, delete-confirmation, and recovery-consent contracts failed | Review controls, explicit confirmation, consent, separate capacities |
| Backup module and lifecycle integration APIs were absent | Archive contracts, capacity/timer policy, durable lifecycle implementation |
| Sparse-copy assertion failed on a filesystem that allocated zero-filled regions | Stream compacts zero regions; physical-hole assertions require supporting source filesystem |
| Replaying a partially restored symbolic link failed | Valid existing links can be safely replayed |
| Disabled settings retained format version 1 | Lifecycle settings persist as version 5, refused by older applications |
| Previous controller health sample survived generation change | Health samples and eligibility invalidated; controller drain persisted |
| Multipass replacement fixture could not stream through the activity wrapper | Opaque archive streaming forwarded without logging archive contents |
| Changed Lima registration was detected after source deletion | Validate registration first, use the supported runtime deletion, then clean owned bytes |
| FIFO and top-level dangling links were omitted/rejected | Special entries and dangling links preserved, including special-file hard links |
| Capacity preflight APIs were absent | Source-volume/reclaim/backup-peak checks and forbidden backup destinations |
| Strict runtime fixture rejected guest execution after VM stop | Stopped storage inspection uses the ownership receipt and read-only host APIs |
| Retired images without a VM incorrectly skipped maintenance | Delete-all review still enters the durable cleanup workflow |
| Cleanup action disappeared after disable; Windows summary showed default 30 GiB | Retired-copy deletion stays accessible; summary uses configured capacity |
| Interrupted backup path was absent from the snapshot and UI | Recovery file, occupied bytes, and verification state remain visible locally |
| An unfinished grow/move operation was classified as missing storage | Existing operation journals take priority over automatic recovery |
| Excluded-location merge contract had no implementation | Later active-pool changes preserve other excluded disks and their restore-capacity action |
| Native acceptance harness failed to compile due to its runner export | Exported native runner; timeout cleanup also stops the owned fixture VM |

The baseline wrapper could not write its external cache, and sandboxed HTTP test
servers could not bind localhost. Checks used installed npm/cargo, a temporary
Cargo target directory, and approved localhost test access. These environment
issues were not treated as passing checks.

The full macOS-host `make check` passed after the implementation and UI corrections:
Python contracts, TypeScript/build, Vitest, workspace Rust tests, formatting, and
Clippy with warnings denied. Final recovery-display and operation-priority changes
rerun this same check.
Native tests remain ignored unless their isolated fixtures are explicitly supplied.

Automated coverage includes the archive round trip and corruption/truncation,
data-fit/recovery policy, disabled-storage persistence, owner interruption,
controller qualification invalidation, original image/volume checks, and runtime
protocol ordering. The complete Multipass protocol fixture interrupts restore,
reopens the application, confirms no second deletion, retries, verifies capacity,
and checks backup cleanup. Missing-disk fixtures cover temporary return, permanent
loss, controller outage, persisted error latching, and owner/consent overrides.

## Native acceptance still required

The following records the original lifecycle implementation. The subsequent
[storage review repairs and validation](storage-review-validation.md) supersede
its macOS arm64 and cross-compilation status; the remaining native gate is still open.

The portable checks do **not** establish native hypervisor, Linux ACL/device-node,
Windows ACL/PowerShell, packaging, or real Kubernetes workload compatibility.
No usable Lima/Multipass runtime and enrolled isolated fixture were available on
this host for the new workflow.

| Target | Status for this workflow |
| --- | --- |
| macOS arm64/Intel, Lima/VZ | Native shrink/removal/recovery untested |
| Linux arm64/x86_64, Lima/QEMU/KVM | Native execution and new cross-platform compilation untested |
| Windows, Multipass/Hyper-V | Native backup/replacement/ACL inspection and compilation untested |
| Windows, Multipass/VirtualBox | Shrink preflight explicitly unsupported |

`crates/agent/tests/native_storage_replacement.rs` provides an ignored native
acceptance entry point for an explicitly enrolled, isolated `nh-storage-lifecycle-*`
fixture. Set `NODEHARBOR_NATIVE_STORAGE_LIFECYCLE` to its settings directory and
`NODEHARBOR_TEST_LIMA` to its native binary when using Lima. Start the owned fixture
with at least 30 GiB, then explicitly run that test with `--ignored`. It shrinks to
half the allocation (and removes extra Lima members), verifies guest contents,
numeric ownership, permissions, links, sparse data and an xattr, and checks the
actual Kubernetes Node capacity. It stops the fixture on completion or timeout;
its retained journal/backup enables inspection after failures.

Before release, run native workloads before/after shrink and member removal, and
fresh workloads after destructive recovery, checking reduced usable capacity.
Also exercise actual unplug/return and wrong replacement volumes, insufficient
remaining capacity/backup space, controller outage, owner Pause/Stop, restart
during every destructive phase, corrupted backups, disk-full restore, and cleanup
interruption. Native failpoints for backup, deletion, verification and cleanup are
still required; fixture-based restore interruption is not proof of all native
crash boundaries. Release packages must identify the exact tested commit/version.
