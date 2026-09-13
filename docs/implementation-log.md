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
- Preparation retries apply changed resource limits before starting an existing
  partial worker; preparing a configured worker retains its normal drain behavior.
- Owner Stop now and installer pause requests interrupt long preparation operations,
  including requests from another process. Immediate shutdown uses Multipass's
  supported force option, retains ownership checks, and reconciles uncertain
  partial-worker states. Normal pauses of running workloads retain their drain.
- Verified runtime downloads are cached by checksum and installed atomically,
  preserving executables used by running processes and surviving failed downloads.
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
- [Run 34730191238](https://github.com/gu1p/nodeharbor/actions/runs/34730191238):
  all five native package smoke checks and container publication passed, including
  real Windows NSIS installation and removal. Later application changes still
  require their own successful builds before release.
- [Run 34731775785](https://github.com/gu1p/nodeharbor/actions/runs/34731775785):
  all five native targets, packaged-app checks, and controller image publication
  passed. The later owner-cancellation change still needs its own native builds.

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
Public TLS and STUN checks passed. Both existing fixed hosts are connected through
the official NetBird client, with explicit peer access policies. Full-MTU packets
passed in both directions, with no loss in the 30-packet checks and approximately
1.2 ms average round-trip time after connection establishment. Kubernetes still
uses its existing private interface; the CNI transition remains separate work.
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
requires the remaining CNI transition and a real enrolled VM.

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

## Remaining acceptance work

- Complete Kubernetes connectivity and the supported CNI/MTU rollout while
  preserving the existing cluster's node addresses and workloads.
- Deploy telemetry, contributed runner pools, and the stateless acceptance workload.
- Complete the explicit disk recreation flow and remaining lifecycle recovery;
  verify runtime binary updates and native installer recovery end to end.
- Publish the first complete release from `main`, verify the one-line installers,
  and confirm each package on its native operating system.
- Enroll a real contributed VM, observe the CI qualification window, run a real CI
  job, and demonstrate recovery. Service admission requires its full 24-hour window.

No live Kubernetes worker has been enrolled by this project yet. GitHub-hosted
builders and pilot packages without OS distribution certificates are approved.
