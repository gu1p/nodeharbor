# Implementation and verification log

This is an implementation record, not a claim that the live fleet is ready.

## Verified in automated checks

Behavioral contracts were written before the corresponding implementations.
`make check` runs the Python release and guest tests, accessible React contracts,
TypeScript compilation, the production UI build, Rust unit and integration tests,
formatting, and Clippy with warnings denied. Windows also runs the PowerShell
installer contracts.

Implemented and covered:

- Tauri desktop and browser fleet UI, enrollment, local sharing controls, schedules,
  idle and battery rules, resource budgets, workload opt-ins, and background settings.
- Atomic local settings, separate device credentials, VM creation receipts, recovery
  from interrupted preparation, and ownership checks before VM operations.
- Guest network/bootstrap setup and a lease watchdog that shuts down an abandoned VM.
- Local drain deadlines that still progress during a controller outage, retrying
  evictions deferred by disruption budgets, and draining before resource changes.
- VM CPU/RAM changes and disk growth after stopping the worker. A smaller disk
  requires an accessible deletion confirmation. The agent drains and stops its
  receipt-owned VM, durably resets old cluster access, and deletes the old disk
  through Multipass. Enrollment survives; replacement preparation is explicit and
  sharing stays off. Retries retain the same request identity across restarts.
- Preparation retries apply changed resource limits before starting an existing
  partial worker; preparing a configured worker retains its normal drain behavior.
- Owner Stop now and installer pause requests interrupt long preparation operations,
  including requests from another process. Immediate shutdown uses Multipass's
  supported force option, retains ownership checks, and reconciles uncertain
  partial-worker states. Normal pauses of running workloads retain their drain.
- Verified runtime downloads are cached by checksum and installed atomically,
  preserving executables used by running processes and surviving failed downloads.
  Existing guests receive the packaged management scripts before configuration;
  services restart through systemd after their verified binaries are installed.
- SQLite controller state, one-use enrollment codes, hashed device credentials,
  trusted gateway authentication, fleet pause/resume, and revocation with retries.
- Independent Kubernetes/NetBird identity and resource verification, a DNS and pod
  network probe, and protected admission labels. Qualification requires real health
  samples; missing time and stale observations do not count as healthy operation.
- CLI update preparation, package checksums, Rosetta-aware CPU detection, and
  Windows application-file restoration after an interrupted installer.
- Complete release manifests, executable version/source identity, signed release
  metadata, immutable publication, and ordering of the latest successful main push.
- Controller images for both Linux architectures, using only verified native
  artifacts and an unprivileged runtime image.
- Private Prometheus metrics and standard OTLP HTTP/protobuf tracing. Request
  attributes use bounded route templates and omit credentials, bodies, device
  identities, raw URLs, and arbitrary incoming baggage. Exporter failure leaves
  the controller available; process tests cover delivery and shutdown flushing.
- Workload drains exclude a probe only after its pod UID and actual DaemonSet
  owner have been verified through the authenticated Kubernetes API. Malformed
  guest workload inventory is an error, not an empty worker.
- Desktop settings register start-at-login before committing a replacement
  request. Failure restores the actual previous OS registration.
- Installers wait for the previous application to exit without killing it. Linux
  thread entries are distinguished from application processes. Linux activation
  publishes the version last and restores launchers after failure; its rollback
  contracts also run on a native Linux host in an isolated temporary directory.
  Windows checks the installed application's exact version and source identity.
  Windows upgrades first obtain a checksum-verified copy of the previous native
  installer. A failed upgrade invokes that installer to restore registration and
  then restores saved application files. Failed native repair is reported
  separately, with the previous files retained for recovery.

## Native build evidence

All five native targets passed checks and packaging in these GitHub runs:

- [Run 34725991343](https://github.com/gu1p/nodeharbor/actions/runs/34725991343)
- [Run 34726473192](https://github.com/gu1p/nodeharbor/actions/runs/34726473192)
- [Run 34727192276](https://github.com/gu1p/nodeharbor/actions/runs/34727192276):
  all native targets and the container smoke test passed. Registry publication
  failed with the upstream BuildKit attestation ordering issue. The builder is now
  pinned to a release containing its fix.
- [Run 34728317373](https://github.com/gu1p/nodeharbor/actions/runs/34728317373):
  all five targets, the container smoke test, and publication of both Linux
  controller images passed after the builder fix.
- [Run 34730191238](https://github.com/gu1p/nodeharbor/actions/runs/34730191238):
  all five native package smoke checks and container publication passed, including
  real Windows NSIS installation and removal. Later application changes still
  require their own successful builds before release.
- [Run 34731775785](https://github.com/gu1p/nodeharbor/actions/runs/34731775785):
  all five native targets, packaged-app checks, and controller image publication
  passed. The later owner-cancellation change still needs its own native builds.
- [Run 34733159697](https://github.com/gu1p/nodeharbor/actions/runs/34733159697):
  all five native targets, package checks, and container publication passed.
- [Run 34734769578](https://github.com/gu1p/nodeharbor/actions/runs/34734769578):
  macOS and Linux passed. Windows exposed a local drain deadline delayed by a
  controller connection attempt. Publication remained blocked by that failed check.

The native macOS ARM application was launched and inspected with actual IPC and
host resource information. Its installer and worker acceptance are separate checks.
New release checks also exercise the packaged applications themselves: app archives
and disk images on macOS, extracted Debian packages and AppImages on Linux, and
silent NSIS installation on Windows. Those checks must pass on their native builders.

## Live deployment status

The private infrastructure repository contains the NetBird deployment and
deployment diagnostics. Private initialization originally succeeded through the
existing GitLab Kubernetes agent; its temporary CI credential was removed. The
initialization workflow was retired after moving management outside Kubernetes.
Public TLS and STUN checks passed. Both existing fixed hosts are connected through
the official NetBird client, with explicit peer access policies. Full-MTU packets
passed in both directions, with no loss in the 30-packet checks and approximately
1.2 ms average round-trip time after connection establishment. The fixed Kubernetes
nodes now use the supported Flannel interface setting over
NetBird, with pod MTU 1230. Their private node addresses and PodCIDR were preserved.
The management service was moved through the supported combined-server/Traefik
Compose deployment to the existing secondary server so it can start independently
of Kubernetes. Database hashes, integrity, existing peer identities and access
policies were verified after the transfer. All active deployments and daemonsets,
plus the Kafka broker through Strimzi, adopted the new pod network and returned
Ready. Cross-node DNS, service routing and intact 512-KiB transfers passed.
Credentials remain outside this public repository. No additional cloud servers
have been created.

The controller now runs on the existing primary with persistent SQLite storage,
restricted credentials, Kubernetes admission policies, and the contributed-node
probe definition. Live admission tests rejected fixed-node changes and broader
bootstrap credentials. The fleet HTTPS route uses the existing Google login;
only enrollment and device endpoints have direct routes to their own authenticated
APIs. Public tests rejected forged identity headers and unauthenticated device
requests. The existing infrastructure portals remained available.

The single private Kubernetes API address is declared through NetBird Networks,
with a TCP-only worker policy. Reapplying the declaration required no API writes.
This is control-plane routing configuration; end-to-end worker connectivity still
requires a real enrolled VM; the fixed-node CNI transition has passed its live checks.

The local Multipass VM boots Ubuntu but has not obtained a DHCP lease. Guest and
host packet captures confirm requests without replies. A test involving the host's
existing VPN settings awaits the owner's specific approval; those settings have
not been changed by NodeHarbor.

Native cancellation was tested separately through the actual agent and Multipass,
with an isolated lifecycle test server. The first test exposed a late hypervisor
change from Stopped to Unknown. The corrected supervisor reconciles that partial
worker through the supported immediate shutdown command. The subsequent native
test passed, including ten further supervisor observations. Its temporary VMs
were removed after verifying they were stopped and had no host directory mounts.
This verifies local cancellation, not fleet enrollment or workload qualification.

Additional regression tests cover an unreachable configured VM when its owner’s
drain deadline expires, and a controller response that omits Content-Length. The
agent now uses the supported immediate shutdown command at the deadline and
bounds streamed responses before accepting enrollment credentials. Full local
checks passed for both fixes. The subsequent Windows deadline failure is recorded
above. The follow-up persists the original drain time, observes it during stalled
network requests and supervisor idle periods, and lets an already-started forced
shutdown finish. Regression checks cover each case, including restart recovery.

## Remaining acceptance work

Controller telemetry has now been deployed and checked against the live
Prometheus, Tempo and Loki APIs. The Grafana dashboard and scoped event collection
are managed by the infrastructure repository. This does not verify an
authenticated browser session or actual contributed worker execution.

The first `main` release passed all five native builds and the container gates,
then stopped during draft publication. A regression test reproduced GitHub's
published-only tag lookup returning 404 for a complete draft. The release tool
now uses GitHub's authenticated, paginated release listing so it can resume and
publish the existing draft without relaxing source identity or asset checks.

- Verify Kubernetes API, pod DNS and MTU connectivity from a real contributed VM.
- Deploy and verify the stateless acceptance workload.
  Contributed runner pools and real no-capacity failures have been validated in
  the infrastructure project; successful contributed execution remains pending.
- Verify worker recreation and runtime updates on real contributed machines;
  complete native desktop and installer acceptance on each supported OS.
- Publish the first complete release from `main`, verify the one-line installers,
  and confirm each package on its native operating system.
- Enroll a real contributed VM, observe the CI qualification window, run a real CI
  job, and demonstrate recovery. Service admission requires its full 24-hour window.

No live Kubernetes worker has been enrolled by this project yet. GitHub-hosted
builders and pilot packages without OS distribution certificates are approved.
