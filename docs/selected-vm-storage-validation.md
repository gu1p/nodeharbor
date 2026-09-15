# Selected storage contains the VM

An owner allocation includes every VM disk. A 100 GiB selection means a 16 GiB
system image and 84 GiB of workload disks, before filesystem overhead. A fresh
30 GiB selection means 16 GiB system and 14 GiB workload. Settings stay in the
application directory. Capacity checks use the selected filesystems, account for
allocated physical blocks in bytes, and reserve 10 GiB once per capacity pool.

Multiple locations retain their independent allocations. The system location
stays stable while it can contain the system image and a data disk. Removing
that location places the system image on another suitable selected location.
Existing larger system images retain their size; they are not silently shrunk.

Legacy configurations that allocated 30 GiB of data plus a separate 16 GiB
system image display their actual 46 GiB total. Saving new rules persists the
rules and storage operation together. It does not start the VM. The owner must
prepare or enable sharing to run pending maintenance. Preparation moves a
stopped owned Lima runtime to the selected filesystem, preserves the original,
verifies the copied image and guest pool, and then removes the original through
Lima. Incomplete copies remain hidden until the VM directory is complete.

Shrink/remove operations also budget their verified backup on its selected
filesystem. Missing system storage cannot recreate a VM on an ancestor volume.
Opt-in destructive recovery budgets its replacement system image inside the
remaining selected allocations and retains cleanup records for unavailable old
VMs. Owner controls, authentication, leases, and health qualification still apply.

## Reproducing native placement and persistence

Provide the Lima executable from the tested package and two distinct mounted,
writable, supported filesystems:

```sh
export NODEHARBOR_TEST_LIMA=/path/to/packaged/lima/bin/limactl
export NODEHARBOR_TEST_STORAGE_PRIMARY=/path/on/selected-volume
export NODEHARBOR_TEST_STORAGE_SECONDARY=/path/on/another-volume
cargo test --locked -p nodeharbor-agent --lib \
  runtime_storage_native_tests::native_selected_vm_moves_grows_and_preserves_data_without_an_enrolled_fleet \
  -- --ignored --exact --nocapture
```

This fixture creates its own private VM and data images. It requires no existing
enrollment or private infrastructure. It writes workload bytes, relocates the
system image, grows the total to 100 GiB, checks actual image placement and
capacity, removes the original VM through Lima, verifies persistence after
restart, adds another selected location, and rejects unavailable system storage.
It preserves failed fixtures and reports their paths if cleanup cannot finish.
The VM must be confirmed stopped before files are copied or deleted. The fixture
does not change the host's VPN, routes, hypervisor installation, or other workers.

The guest filesystem is smaller than its virtual disk because of metadata.
For example, Ubuntu reported an 84 GiB data device and 88,582,103,040 bytes of
ext4 filesystem capacity. Verification checks the exact virtual image size and
the existing guest-capacity qualification bounds separately.

`scripts/desktop_storage_acceptance.py` exercises the exact packaged Linux GUI
through AT-SPI in an isolated display and configuration. It checks multiple drive
selections, allocations, editable directories, navigation, combined save, and
reopening. Its evidence records the package's commit and version. Native VM
qualification is a separate check; GUI-only evidence never claims a VM boot.

## Regression checks recorded before fixes

- The accessible review described 100 GiB of workload storage plus another
  system image, instead of a 100 GiB total.
- The backend rejected a selected 100 GiB allocation with 256 GiB available
  because the unrelated settings filesystem had only 25 GiB free.
- The initial layout contracts lacked total/data conversion, stable system
  placement, and fractional physical-block capacity credit.
- Runtime copy and image-proof contracts lacked volume/owner validation, sparse
  byte preservation, wrong-image-size rejection, and cancellation handling.
- An interrupted copy exposed a partial VM directory; retrying also replaced an
  already verified image rather than reusing it.
- Recovery allocated its remaining drive entirely to workload data without
  including the replacement system image.
- Shrink review accepted 30 GiB total plus a 2 GiB backup and a 10 GiB reserve
  with only 40 GiB available. The combined requirement is 42 GiB.
- Finishing a legacy maintenance journal counted its 16 GiB system allowance
  twice when reopening effective resource settings.
- The packaged GUI verifier compared physical data allocations directly to
  total owner selections and omitted the saved system placement.
- macOS protocol fixtures exceeded Lima's Unix socket path limit under the
  normal per-user temporary directory. The fixtures now select short paths.
- CLI supervision overflowed a 1 MiB process stack, reproducing the Windows
  startup failure. Supervision now keeps its large operation futures on the
  heap; a process-level stack-limit test also runs on Unix.
- Reopening a saved growth request displayed the previous allocation while
  maintenance was pending. The editor now shows the saved target separately
  from the currently applied allocation, without granting active capacity.
- A Linux folder at the QEMU user-network socket boundary passed review and
  failed when starting the packaged VM. Review now includes QEMU's longer
  socket name and rejects that folder before writing configuration or VM files.

Run `make check` for the behavioral, accessibility, unit, integration, and lint
checks. Release acceptance additionally requires the desktop matrix, signed
Android qualification, exact-package native checks, and agreement between the
published downloads and update channel. Local checks alone do not establish
those results. Detailed development logs remain under ignored
`.local/selected-vm/`; release evidence must identify the exact tested commit.
