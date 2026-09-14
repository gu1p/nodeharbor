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

Dependency: task 724 is in the separate `multiple-disks` worktree. At the start
of this task it contained contracts only, with no committed storage implementation.
Integration must use its reviewed storage model/runtime rather than inventing a
second attachment path or changing global host storage/VPN settings.

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

Review status and remaining acceptance:

- Implemented local consent/CLI, durable versioned requests, authenticated
  controller/agent exchange, browser editing and audit comparison, revocation
  during delivery/drain/application, duplicate handling and offline reporting.
- CPU/memory and contribution-rule edits reuse the owner policy validator and
  owned VM lifecycle. Lima disk growth uses the managed disk's filesystem for
  capacity checks. Multipass disk growth requires local approval because its
  daemon does not expose the host storage location through the supported API.
- Disk changes are never considered applied merely because they were queued.
  Unknown allocation after a failure or interruption blocks sharing. The supported
  in-app recovery is the locally confirmed worker replacement, which explains
  deletion before proceeding. There is no remote recovery or deletion shortcut.
- Gateway-authenticated requests record the administrator's allowed email. The
  existing shared administrator token is recorded as “administrator token”; it
  cannot identify an individual person. The UI shows the latest 100 requests;
  controller audit history remains persisted, and the agent retains 64 receipts.
- Task #724 has no reviewed commit available in this worktree. Its current
  investigation reports no supported API for placing individual disks on selected
  host volumes with the pinned runtimes. Adding disks, relocating disks, and
  integrating its final storage model remain incomplete acceptance criteria.
  No work was imported from another worktree, merged, committed or released.
- The automated suite runs on this macOS host. A Chromium smoke with fixture API
  responses checked the browser form, focused cancellation, confirmation, posted
  revision/values and requested state; its confirmation screenshot was inspected.
  Windows, Linux, real VM resizing, deployed gateway sessions and real worker
  health qualification have not been validated for this change. VM/controller
  integration tests use isolated fixtures and localhost, not live fleet devices.
