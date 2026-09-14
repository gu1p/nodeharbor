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

After picking drives and allocations, choose **Save sharing rules**. Review the
locations, allocations, changed rules, and restart or backup requirements, then
choose **Confirm and save**. Both the policy and storage request are validated
and committed together. Existing-worker maintenance runs afterward; its status
is separate from confirmation that the settings were saved.

Unfinished disk, folder, allocation, and policy edits survive page navigation.
Canceling review keeps the draft. **Discard storage changes** restores the saved
storage without discarding CPU or scheduling edits; **Reload current settings**
explicitly reloads the entire draft. Background polling preserves drafts, and
conflicting revisions require reloading before committing. Prepare and Start
return to unfinished edits instead of using an older allocation.

A disabled button no longer displays a loading cursor. Reviewing and saving
have explicit progress states. Owner Pause and Stop controls remain available
while a combined save is pending.

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

## Combined-save regression evidence

The prior disabled-save contract was incorrect: it asserted that the owner's
primary action should become unusable after adding a disk. Its replacement
first failed on disabled Save, lost navigation drafts, preparing old storage,
missing save-error recovery, and missing conflict handling. Draft unit contracts
and combined agent integration contracts were then added before implementation;
the absent helper and agent API were recorded as failing checks. A further
behavioral check exposed inaccessible owner controls during a pending review;
the review now includes Pause and explicitly confirmed Stop.

The combined native save carries a policy, its expected configuration revision,
and the reviewed storage plan. Existing policy-only calls and remote storage
operations remain compatible. Rejected validation commits neither policy nor
storage. The existing supervisor owns VM changes, backup, restore and capacity
qualification; a settings-save acknowledgment does not claim these finished.

`npm --prefix ui run e2e` runs the complete two-drive keyboard setup in Chromium
and WebKit. The fixture imports the production App, persists only fixture
settings, and requires both disks before simulated preparation. On macOS the
WebKit test uses Option-Tab, respecting the host's keyboard-navigation setting.
The Linux release job runs both browser engines as a required check.

These browser and agent fixtures do not qualify native VM operation. Packaged
GUI and native Lima acceptance must be recorded separately for the exact
release commit and version.

The reusable `scripts/desktop_storage_acceptance.py` drives a packaged Linux
application through AT-SPI and XTest in an isolated D-Bus/Xvfb session. It uses
temporary configuration and directories on explicitly supplied mounted volumes,
checks the executable version and commit, saves through the real GUI, and reopens
the application to verify persistence. It never enrolls or starts a worker.
Its initial run against v0.2.80 reproduced “Adding a disk disabled Save sharing
rules.” Pass `--help` for the package paths, volume roots, allocations and evidence
arguments. Native VM capacity and workload persistence still require the
separate native Lima tests.
