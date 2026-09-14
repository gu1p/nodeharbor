# Selected-drive capacity and setup validation

Workload storage belongs to the drives selected in **Storage locations**. Each
location's allocation is checked against its mounted filesystem. Allocations
sharing one capacity pool are counted together, with 10 GiB kept free once per
pool. Existing disk images receive credit only for their actual allocated blocks.

The application's volume has a separate system-disk allowance. A fresh Lima
worker reserves 16 GiB for that disk, plus 10 GiB free space. For example, 100 GiB
on another drive with 110 GiB free is valid when the application volume has
27 GiB free. The application volume does not need to fit those 100 GiB. If both
disks use the same capacity pool, their allowances are counted together.

After picking drives and allocations, choose **Review storage changes**, check
the locations and amounts, then **Apply storage changes**. Sharing rules can be
saved after storage is applied. Unsaved storage edits are announced and keep the
sharing-rules save disabled; **Discard storage changes** restores the saved
choices without losing CPU, memory, or scheduling edits.

Previously, saving sharing rules while a drive selection was still a draft
validated the old default allocation on the application volume. The error
combined the minimum allocation and free-space reserve, even for a 100 GiB
selection. A second path hid selected-volume failures by converting the failed
storage observation to zero capacity before validating the policy.

The fix preserves the actual storage error. A selected-drive shortage identifies
its mount, additional space required, and capacity available after the reserve.
A separate system-disk shortage identifies the application mount and remaining
system-disk requirement. Allocations below 15 GiB receive a distinct minimum-size
error.

## Regression evidence

The failing contracts were recorded before implementation, in this order:

1. UI behavior: a pending 100 GiB drive selection left **Save sharing rules**
   enabled without an accessible explanation, and had no discard action.
2. Unit checks: minimum-size and free-space errors were indistinguishable;
   selected-drive and system-disk errors omitted the mount and capacity figures.
3. Agent integration: saving rules after a selected drive lost capacity returned
   the generic 15 GiB message instead of the selected-drive error.

The integration suite also exercises 100 GiB on one selected drive and 60 + 40 GiB
on two drives with only 27 GiB free on the application volume. It checks review,
application, reopening, saving sharing rules, and requesting preparation. It
preserves independent allocations and the separate 16 GiB system allowance.
The fixtures use explicit volume inventories and a VM runner that rejects every
command; these checks neither start a VM nor need a private deployment.

Run the full checks with `make check`. Focused reproductions are:

```sh
cargo test --locked -p nodeharbor-core --test policy_contract
cargo test --locked -p nodeharbor-agent --test storage_allocation --test storage_changes
cd ui
npm test -- src/StorageLocations.test.tsx
npm run e2e
```

The agent integration checks also run through `make acceptance`. For uncommitted
development changes, use `python3 scripts/acceptance.py --allow-dirty`; its report
explicitly identifies simulated infrastructure and cannot qualify a release.
