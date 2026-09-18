# Task 724: storage locations and combined worker capacity

## Implemented behavior

On macOS and Linux, the owner can choose one or more folders and a GiB allocation
for each worker disk. Sharing rules shows volume labels, mount locations,
filesystems, available and configured capacity, a native folder picker, and a
review before applying. With no selection, preparation persists a 30 GiB data
disk under the displayed application-managed `storage` directory.

The worker has a separate 16 GiB system disk. Existing workers retain their
previous system-disk allowance when converted. The inventory shows this disk
separately; its remaining possible growth is reserved on the application volume.
The planner leaves another 10 GiB free per host capacity pool. Shared APFS volumes
are counted together. Only real allocated image blocks receive free-space credit;
a sparse image's logical length is never treated as reserved host space.

The selected folders contain private, receipt-owned image directories. Lima
attaches the images through `additionalDisks`. Inside the VM, LVM combines them
into one linear logical volume formatted as ext4. K3s/containerd data, the kubelet
root, and Pod logs use that filesystem. Harbor Build's per-machine cache path,
`/var/lib/harbor-build`, is a link into that same filesystem, made whenever the
pool is activated: at every boot and every worker configuration, so a worker
updated in place gains the link in the same session. This keeps the cache off the
16 GiB system disk. If the link cannot be made, the worker still starts and the
guest log says why. The cache shares the pool with container images, Pod logs and
build scratch space, and the kubelet's disk-pressure limit (15% free) applies to
their combined use; the cache sizes itself from free space when it starts, so on a
small allocation it leaves less room for builds. Kubernetes reports its actual capacity
and usage. Controller health now rejects a node whose measured capacity is below
85% or above 100% of its disk allowance, or whose allocatable capacity is missing,
zero, or greater than capacity.

Owners can add disks, grow existing disks, or move an existing disk to another
folder/volume. The app records a durable operation, asks the controller to stop
assignments, drains work, stops the worker, changes images and attachments,
checks the guest pool, and returns to the owner's sharing rules. The drain deadline
can interrupt work; this is stated before applying. Pause and Stop now remain
available. Guest preparation renews the existing owner lease and stops if it is
lost. Offline shrinking/removal and optional missing-disk recovery are implemented
by the follow-up [storage lifecycle workflow](storage-lifecycle-validation.md).
The native results below cover the original add/grow/move scope only.
Subsequent repairs and platform results are recorded in the
[storage review validation](storage-review-validation.md).

Moves preserve the original image until the replacement has been checked.
Any remaining old copies are persisted and displayed; cleanup resumes while the
worker is stopped and the original volume is available. Existing host files are
never imported, formatted, or treated as owned disks. Existing guest data is copied
with sparse-file, ownership, mode, ACL and extended-attribute preservation and
checked before switching paths.

Saved disk IDs, volume IDs, allocations and pending operations survive application
restarts. Startup checks the ownership receipt, private file permissions, actual
filesystem identity and Lima registration. A missing or replaced volume blocks
the pool; it is never redirected to the system disk. Guest boot activates only the
complete saved pool and never formats missing disks or falls back to old storage.
Combining disks provides capacity, not redundancy: losing one member makes the
whole worker pool unavailable.

Windows keeps Multipass. Inventory and single-disk controls remain
available; verified replacement requires Hyper-V storage inspection as described
in the lifecycle notes. Selecting an individual disk's folder, moving it and adding disks are
explicitly unavailable. NodeHarbor does not change the Multipass daemon's global
storage location or manipulate its VM behind its configuration.

## Runtime boundary

The bundled runtime is pinned to Lima 2.2.0 and the guest/image versions recorded
in `runtime/lima.json` and the existing runtime lock. macOS uses VZ; Linux uses
QEMU/KVM. Both use native CPU architecture, application-owned networking, no host
folder mounts, and no personal SSH keys. Existing VPN policy, host networking and
host filesystem ownership settings are preserved.

Lima's public disk commands create/inspect/resize registered disks, and its
`additionalDisks` configuration attaches them. It has no first-class arbitrary
host-path field for additional disks. This implementation places the entire
registered disk directory through a directory link into its selected private
folder. That behavior is verified against pinned Lima source and native macOS
lifecycle tests; it remains a compatibility dependency to requalify on runtime
updates and on native Linux. A symlink to only `datadisk` is deliberately avoided
because image-format conversion can replace that file.

Plain mode disables Lima's automatic additional-disk formatting and mounting.
NodeHarbor's guest helper owns initialization, a persistent recovery journal,
partition/PV/filesystem identities, complete-pool activation and migration.
Repeated committed requests and interrupted journals are replayed by exact request
digest. An unrelated request cannot replace a pending operation.

The shared snapshot, preview and apply contracts expose the inventory and
configuration for an opted-in owner configuration transport. This repository does
not yet contain that remote transport. Host paths and volume IDs are not added to
ordinary heartbeats. End-to-end remote configuration is not claimed.

## Verification and recorded failures

Tests followed the requested order: behavioral/accessibility contracts, allocation
units, integration contracts, then implementation. `make check` was invoked after
each change. Intermediate failures were retained rather than counted as passes:

| Stage | Observed failure before its fix |
| --- | --- |
| UI contracts | Three missing behaviors: moving without removal/shrink, combined capacity explanation, review/apply flow. |
| Allocation units | Stable disk IDs and ext4/XFS were rejected; combined minimum was absent. |
| Integration | Preview rejected all selectable storage; initial prepare did not persist/use a data disk. |
| Owner lifecycle | Missing receipt allowed VM inspection; replacement downgraded saved settings; Pause did not pause storage maintenance; changed restart impact was not rejected. |
| Lease handling | Storage package preparation did not renew the owner lease or end on rejection. |
| Space accounting | System-image growth was omitted from the selected pool's budget; the current Lima `disk` file also needed physical-block credit. |
| Disk lifecycle | Wrong-volume writes/cleanup, changed links, insecure or hard-linked images, and interrupted move/growth retries needed explicit guards. |
| Guest native pool | LVM returned one row per linear segment; the initial check incorrectly rejected a valid multi-disk LV. Journal recovery preserved the images, then a fresh boot passed. |
| Guest configuration | Missing pool state could fall back to legacy paths; configuration now stops before service/network changes. |
| Storage permission | Heartbeats still evaluated capacity beside the settings file; they now use validated selected capacity without adding host locations to the payload. |
| Boot recovery | An interruption after adding an LVM member blocked the old manifest at boot; exact committed replay could omit its service. Owned journal replay now completes before final activation, and installs the service before committing state. |
| Controller health | A healthy 15 GiB boot filesystem was accepted for a 30 GiB worker; allocatable was not validated. |
| Native proof harness | Pod creation raced its service account, then checksum readiness raced an unfinished checksum. Both harness errors were fixed and the complete test rerun. |
| Check environment | The sandbox blocked localhost test listeners. The required suite was rerun with local TCP access. Default shared cache writes were blocked; task-local cache/target paths were used. |
| Existing tests | The guest bundle file-count assertion needed the new storage helper. Old blanket Lima-unsupported assertions were updated to the implemented runtime boundary. |

Native macOS 26.6.1 Apple Silicon results with Lima 2.2.0/VZ, pinned Ubuntu
24.04 and K3s 1.36.4+k3s1, and two different APFS volumes:

- Create, inspect, grow and reopen file-backed disks: passed.
- Clean VM provisioning, legacy test-file ownership/mode/content preservation,
  combined ext4 capacity and filesystem UUID across VM restart: passed.
- Add a disk, grow a member, move a disk across volumes, then restart with unchanged
  data and filesystem UUID: passed.
- Hide an owned backing directory while stopped: rejected without changing the
  registered location; restoring it permitted restart. This is not a physical
  hot-unplug test.
- Kubernetes advertised **16,788,615,168 bytes capacity** and **15,109,753,627 bytes
  allocatable** from two 8 GiB members. An ordinary Pod requested 10 GiB and wrote
  **9,663,676,416 bytes (9 GiB)** of random data, larger than either member.
  Kubelet Pod usage and matching node/image filesystem capacity were checked.
  The full file's SHA-256 matched after a normal VM stop/start. Guest clock skew
  was 0.13 seconds at the initial check.

The native proof predates the final boot-journal recovery fix. That fix is covered
by mocked interruption, changed-journal, active-worker and service-repair tests;
its crash/reboot path has not been rerun in a native VM. The complete Python, UI,
Rust, formatting, build and clippy suite passed before that final guest change;
the final `make check` result is reported with this review.

The Kubernetes proof is repeatable through `tests/native_storage_kubernetes.py`;
it requires explicit opt-in, an owned test fixture, and rejects enrolled worker
configuration. The fixture contains a standalone test cluster, not a fleet worker.
Disk/VM opt-in tests live in `crates/agent/tests/native_lima_storage.rs`.

An existing external disk had ownership disabled and was rejected. Its settings
were left unchanged. Native positive tests used a new task-owned APFS image on
that disk with ownership enabled only for the new test volume. Temporary VMs and
fixture data are cleaned up after verification; logs remain outside the worktree.

## Platform status and remaining limits

| Host / storage | Implementation | Verification |
| --- | --- | --- |
| Apple Silicon macOS, APFS with ownership enabled, Lima/VZ | Folder selection, multiple disks, add/grow/move, combined Kubernetes capacity | Native disk, VM and workload proof passed on this host. |
| Intel macOS, APFS with ownership enabled, Lima/VZ | Same storage path; native architecture and runtime preflight | Native Intel hardware untested. |
| Linux x86_64, local ext4/XFS, Lima/QEMU/KVM | Same storage path; QEMU >=6.2 and accessible KVM required | Agent cross-compilation and portable contracts passed; native KVM and Linux desktop/package execution untested. |
| Linux arm64, local ext4/XFS, Lima/QEMU/KVM | Runtime packaging/configuration included | Native and cross-compilation untested. |
| Windows / Multipass | Inventory, explicit capability message, existing disk-size controls | Native Windows behavior and packaging untested. No per-instance location/additional-disk support. |
| Ownership-disabled APFS, exFAT, network filesystems, other host filesystems | Rejected | APFS ownership rejection tested natively; other filesystem boundaries covered by fixtures. |
| Linux without accessible KVM/QEMU, cross-architecture VM, other hosts | Rejected by preflight | Portable capability contracts; no host-setting changes. |

The pinned Intel guest image contains Linux 6.8. Lima warns that an Intel guest
with Linux 6.12 or newer needs macOS 15.5 or newer; recheck this constraint when
updating the image. [Pinned VZ driver](https://github.com/lima-vm/lima/blob/v2.2.0/pkg/driver/vz/vz_driver_darwin.go#L257),
[pinned image manifest](https://cloud-images.ubuntu.com/releases/noble/release-20260705/ubuntu-24.04-server-cloudimg-amd64.manifest).

Physical host reboot, actual unexpected volume unplug, a real fleet's storage
migration/drain/requalification, and disk-full stress under competing host writes
remain untested. Permission/free-space checks cannot reserve a shared disk against
unrelated host applications. There is no online pool shrink/removal or redundancy.
Offline replacement and opt-in recovery now have separate automated contracts;
their native acceptance checks remain pending. Conversion retains original guest
data directories on the system disk after verification. Existing Multipass workers retain
their provider; no Linux data migration was requested.

This worktree is for review. No release, merge or publication was performed.

## Upstream references

- [Pinned Lima disk schema](https://github.com/lima-vm/lima/blob/v2.2.0/pkg/limatype/lima_yaml.go),
  [disk commands](https://github.com/lima-vm/lima/blob/v2.2.0/cmd/limactl/disk.go), and
  [disk store](https://github.com/lima-vm/lima/blob/v2.2.0/pkg/store/disk.go).
- [Lima plain mode](https://lima-vm.io/docs/config/plain/) and
  [VZ runtime requirements](https://lima-vm.io/docs/config/vmtype/vz/).
- [Kubernetes local ephemeral storage](https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/#local-ephemeral-storage)
  documents the supported storage layout and accounting constraints.
