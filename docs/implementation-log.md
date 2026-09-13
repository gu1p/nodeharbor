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
- VM CPU/RAM changes and disk growth after stopping the worker. Disk shrinking
  remains rejected until the explicit recreation flow is implemented.
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

The native macOS ARM application was launched and inspected with actual IPC and
host resource information. Its installer and worker acceptance are separate checks.
New release checks also exercise the packaged applications themselves: app archives
and disk images on macOS, extracted Debian packages and AppImages on Linux, and
silent NSIS installation on Windows. Those checks must pass on their native builders.

## Live deployment status

The private infrastructure repository contains the NetBird bootstrap deployment,
restricted initialization workflow, and deployment diagnostics. The network server
is running on an existing cluster node. Private initialization succeeded through
the existing GitLab Kubernetes agent; its temporary CI credential was removed.
Public TLS routes and cloud peer connectivity are being deployed. Credentials remain outside this public
repository. No additional cloud servers have been created.

The local Multipass VM boots Ubuntu but has not obtained a DHCP lease. Guest and
host packet captures confirm requests without replies. A test involving the host's
existing VPN settings awaits the owner's specific approval; those settings have
not been changed by NodeHarbor.

## Remaining acceptance work

- Complete public TLS activation, cloud peer connectivity, routing/ACLs, and MTU
  configuration while preserving the existing cluster's node addresses and workloads.
- Deploy the controller, gateway routes, admission policies, probe, telemetry,
  contributed runner pools, and the stateless acceptance workload.
- Complete the explicit disk recreation flow and interrupted-operation handling;
  verify runtime binary updates and native installer recovery end to end.
- Publish the first complete release from `main`, verify the one-line installers,
  and confirm each package on its native operating system.
- Enroll a real contributed VM, observe the CI qualification window, run a real CI
  job, and demonstrate recovery. Service admission requires its full 24-hour window.

No live Kubernetes worker has been enrolled by this project yet. GitHub-hosted
builders and pilot packages without OS distribution certificates are approved.
