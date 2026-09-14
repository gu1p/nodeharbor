# Task 726: owner-approved remote configuration

Behavioral contracts, recorded before implementation:

- Remote configuration starts disabled on every node, including upgraded settings.
  Only a local owner control can grant or revoke it. Consent describes resource,
  storage, schedule, power and workload changes and their interruption risk.
- Revocation is independently saved. It invalidates requests still in transit or
  draining; re-enabling consent cannot revive an old request.
- The authenticated dashboard can inspect consent, revision, supported options,
  reported settings and capacity. Paths are sent only with owner consent.
- Requests bind a unique ID, expected revision, authenticated administrator,
  timestamp and explicit interruption acknowledgment to a complete settings edit.
  Only one request may be outstanding. Duplicate retries return the same result;
  reusing an ID with different content and stale revisions are conflicts.
- The node revalidates consent, revision, physical capacity, native permissions
  and supported runtime options before mutation. Local edits invalidate pending
  requests. Neither delivery nor persistence is an applied acknowledgment.
- Running work drains under the owner's deadline before VM resource changes.
  VM failure remains a rejection with an actionable error and honest effective
  allocation. Unknown or partially applied allocation must prevent sharing.
- Offline is separate from the last request outcome. Requests cannot be newly
  queued against stale inventory. Existing requests remain visible after restart.
- Audit history records actor, request ID, time, before/after settings and outcome.
  Enrollment, owner receipts, isolation, qualification and VPN policy still apply.

Accessible browser and desktop contracts:

- A named local consent checkbox is unchecked by default, has a description and
  saves independently of the sharing-rules form. Failed changes are announced.
- Each fleet node has a named configuration action. Its editor exposes labeled
  resource and sharing controls, hardware/capacity and capability explanations.
- Disabled consent and offline inventory prevent submission. Unsaved values and
  failures survive refresh. A stale edit requires explicit reload of current data.
- A keyboard-accessible confirmation explains drains/restarts before submission.
  Requested, pending, applied and rejected outcomes are announced with actor/time.
- Native start-at-login changes need local approval. Unsupported storage options
  are explained rather than accepting a request the runtime cannot implement.

Initial dependency state: task 724 had not yet supplied a committed storage
implementation when the foundation was written. The later integration below uses
its reviewed local-main model and runtime; no second attachment path was added.

Verification:

- Baseline `make check`: 67 Python tests passed (3 platform skips); npm was blocked
  by sandbox access to the existing shared cache. Retried with cache access using
  the approved `make check` command. No host settings or permissions were changed.
- Baseline completed successfully on this macOS host, including Rust tests and
  clippy; native VM acceptance and other OSes were not exercised.
- UI red: `make check` failed all 5 new remote-configuration tests (39 existing
  tests passed): missing local consent and per-node Configure controls.
- Unit red: `cargo test --locked -p nodeharbor-core --test remote_configuration`
  failed with E0432, missing `nodeharbor_core::configuration`.
- Controller integration red: all 3 new tests failed with missing routes (404
  instead of authentication errors or 202). Agent integration contracts were then
  added for late delivery after revocation and failed runtime disk changes.
- Agent integration red: missing remote report/consent/receive APIs and state
  fields produced compiler failures before implementation.
- First Rust implementation check: 3 authorization/versioning unit tests, 5 agent
  tests and 3 controller integration tests passed.
- UI implementation check: 43 tests passed; one assertion matched both the request
  notice and history status. Tighten that assertion to the exact history status.
- Lifecycle regression red: the supervisor's deadline shutdown canceled the
  request it was draining for (7 agent tests passed, 1 failed). Separate explicit
  owner actions from supervisor bookkeeping in revision advancement.
- Full `make check` passed after the lifecycle fix. The real controller/agent
  delivery and applied-acknowledgment test also passed.
- A follow-up accessible UI contract exposed a stale “Requested” notice remaining
  after an applied acknowledgment. Status now comes from the refreshed request
  history, whose setting changes are presented as a readable comparison table.
- Concurrent snapshot regression red: a local snapshot paired an older policy
  with a newer revision under concurrent writes. The policy and report must use
  the same atomic settings read. The stale replacement-confirmation contract also
  failed with the missing versioned replacement API before that API was added.
- Physical-storage capability red: the agent/controller accepted disk growth for
  Multipass despite its unknown daemon storage directory; the browser also left
  that disk field editable. Restrict browser disk growth to the application-managed
  Lima disk, report the capability, and recheck its actual filesystem capacity.
  The runtime disk-failure fixture now uses Lima, where remote growth is eligible.
- Inventory UI red: the browser did not show the resolved worker disk path or
  per-volume configured allocation. Add those read-only, consent-gated fields.

## Current behavior and platform limits

- Rust desktop/server agents expose every contribution policy field supported
  locally, with start-at-login explicitly requiring native local approval.
  Owner consent remains exclusively local and is disabled by default.
- The browser reuses the local Storage editor and strict local storage schemas.
  Node-generated previews cover new file-backed disks, selected directories,
  growth/moves, backup-based shrink/removal, deletion, returned-disk restoration,
  recovery preferences and maintenance retry. Application requires a retained
  node-issued plan at the same revision and explicit interruption acknowledgment.
- Durable storage phases use the same VM ownership, capacity, isolation, backup,
  verification, generation and health-qualification paths as local operations.
  Scoped settings writes and controller/runtime commands check current authority.
  Revocation clears unstarted operations or pauses interrupted journals. Partial
  files or verified backups can remain in reviewed locations; they are preserved
  for inspection/retry and never silently redirected to another volume.
- Storage remains pending until the node verifies it. Read-only previews report
  Reviewed. Reports distinguish configured capacity from active storage and omit
  verified allocation during maintenance or unknown compute allocation.
- Consent, node identity/runtime, local policy, storage choices, recovery preferences,
  explicit owner actions and application-maintenance holds share version checks.
  Internal remote storage phases advance revisions without overwriting owner edits.
  Format 6 prevents old agents from bypassing authority on remote storage journals;
  older local-only configurations remain supported and consent stays off on upgrade.
- Gateway-authenticated requests record the administrator's allowed email. The
  shared administrator token is recorded as “administrator token”; it cannot
  identify an individual person. History shows before/requested/effective settings,
  paths, actor, times and errors. The UI shows 100 requests; the agent retains 64
  receipts, while the controller's audit history remains persisted.
- Additional disks/locations use Lima on supported macOS/Linux hosts. Windows
  Multipass retains the local single-disk review/replacement and its supported
  native capacity inspection. Native OS approvals and runtime installation remain
  local. The separate Android development agent has no remote configuration
  protocol; its dashboard status explicitly reports that support is unavailable.
- No host network settings, VPN configuration or routing exceptions are changed.
  Live VM/backup/replacement acceptance, Linux/Windows hosts, Android runtime
  behavior and deployed gateway sessions have not been exercised for this task.
  Runtime/controller integration uses isolated fixtures and localhost.

## Integration with local main (2026-09-14)

Rebased foundation onto local main `df0aac4`, including the reviewed multi-disk
storage lifecycle (`a398fa9`) and Android platform changes. The rebased baseline
passes `make check` on macOS (73 UI tests).

New behavioral/UI contracts, before storage transport implementation:
- The browser exposes the same labeled directory, allocation, backup folder,
  shrink/remove, restore, recovery preference, retry, and deletion review controls.
- Review runs on the authenticated node, checks physical capacity, and returns a
  versioned plan. Applying requires the exact reviewed plan and interruption
  acknowledgment; deletion additionally requires its explicit data-loss control.
- Pending disk changes remain pending until runtime verification; failures and
  revocation pause interrupted maintenance and report preserved/recovery state.
- Local storage edits and owner recovery choices invalidate stale remote requests.
- Pooled CPU/memory changes leave the separate worker system-disk size unchanged.
- Storage UI red: `make check` has 2 new failures (73 passing): missing Add storage
  location and recovery controls in the browser editor.
- Unit red: core storage operation deserialization rejects unknown `operation`;
  agent recovery-policy edit leaves the pending remote command intact.
- Controller–agent integration red: opted-in report has no shared storage inventory
  (`storage.supported` is null). No storage command is accepted or applied yet.
- First storage implementation compile check found two helper methods private to
  the policy module. Limit their visibility to the shared agent parent module.
- First complete storage check: browser and controller–agent storage tests pass;
  the unit contract catches Serde accepting extra fields on a tagged unit variant.
  Represent the retry action as a strict empty struct variant.
- Runtime integration passes staged-request revocation, failed disk creation, full
  multi-disk verification, and compute changes without changing the system disk.
  The stricter boundary check fails: a disk inspection command continues after
  revocation, before the periodic cancellation future runs. Gate every runtime
  command and controller action on current consent, before and after awaiting it.
- Retry integration red: the shared Retry storage maintenance API rejects paused
  growth/add/move operations. Extend that same local API to resume their durable
  journal; remote retries still need fresh consent and wait for verification.
- UI regression red: a local storage save leaves an otherwise valid sharing-rules
  draft on its old revision. Only merge the new disk allowance/revision when all
  other saved policy fields still match the draft's original base. A concurrent
  unrelated edit must keep the draft stale. Nodes lacking the protocol also need
  a separate unsupported message instead of desktop consent instructions.
- Compatibility unit red: owner consent leaves the settings in an older format
  whose agent cannot enforce remote authority on storage journals. Remote use now
  promotes settings to format 6, which older agents explicitly reject; revocation
  and upgrades retain that format. Existing local-only formats remain readable.
- Status/audit red: read-only previews are labeled Applied, and the storage history
  lacks its before/requested/effective comparison. Return Reviewed for previews
  and render each changed directory, allowance, removal, and recovery preference.
- Capacity integration red: a staged storage journal reports the previous allocation
  as verified. Report no verified allocation and zero active storage during storage
  maintenance or unknown compute allocation, while retaining configured choices.
- Audit UI check: the new Requested column also matches the old status assertion.
  Scope the assertion to the history status; both remain visible to users.

## Verification of the integrated implementation

- `make check` passes on macOS: 149 Python tests (3 platform skips), 80 accessible
  UI tests, 290 Rust tests (13 native/environment acceptance tests ignored),
  formatting, clippy with warnings denied, TypeScript and the production UI build.
- Controller–agent integration covers storage preview/application/recovery over
  real authenticated localhost HTTP, duplicate replay, over-capacity rejection,
  offline rejection, revocation and acknowledgment/audit propagation. Separate
  controller coverage preserves pending state across storage phase revisions.
- Injected Lima integration covers fully verified multiple-disk application,
  preserving the system disk on CPU edits, revocation before and between disk
  operations, failed creation, paused recovery and a fresh remote retry.
- Chromium smoke with fixture API responses exercises asynchronous preview polling,
  interruption review, the exact versioned plan submission, requested/applied
  acknowledgment and the before/requested/effective storage audit. Both review
  and applied screenshots were inspected. This does not substitute for live VM
  or deployed authentication acceptance.
- No release was manually published. The existing main-push workflow automatically
  publishes images/releases after successful CI; that behavior must be reconciled
  with the earlier no-release constraint before a main push.
- The documentation/layout verification run hit a timeout in the existing
  `an_earlier_worker_error_does_not_reject_a_new_update_before_inspection` fixture
  (the isolated setup tick exceeded its three-second deadline). Investigate and
  rerun before considering the final source verified.
- The complete rerun passed without weakening that timeout. Snapshot reports now
  reuse the already-discovered inventory, including its volume capacity, instead
  of repeating host volume scans during each snapshot; local and remote views use
  the same storage observation. The subsequent full check validates that change.
- The CLI replacement action also passes the revision from its settings read to
  the already-tested versioned replacement API, matching the desktop's conflict
  protection. No local confirmation or native approval is bypassed.
