# Worker networking recovery plan

NodeHarbor must prepare a Linux worker on the owner's current computer while
preserving the host's VPN configuration and connection. A released installer and
passing unit tests do not establish that a real worker can join the fleet.

## Evidence and decision gate

The existing macOS Multipass worker boots but remains unreachable. Captures show
DHCP requests on its virtual bridge without replies. Starting Apple's DHCP
service did not resolve the failure. This does not establish VPN interference.

Evaluate Lima's supported `user-v2` network with Apple's Virtualization framework
on macOS. This uses application-managed networking instead of depending on the
host's virtual-bridge DHCP service. Do not switch Multipass's global driver,
replace its binaries, add static routes, edit packet-filter rules, or change VPN
settings as an application workaround.

The isolated Apple Silicon evaluation uses the official Lima 2.2.0 distribution
and Ubuntu 24.04 image, both checked against pinned SHA-256 digests. It has two
CPUs, 2 GiB memory, an 8 GiB disk, no shared host directories, no imported personal
SSH keys, and no forwarded SSH agent. It is separate from the enrolled worker and
contains no fleet credentials.

Initial results on the affected Mac:

- The guest booted, SSH became available, and Lima reported ready.
- Guest DNS and HTTPS requests succeeded.
- The fleet and private-network API endpoints were reachable over verified TLS
  and correctly rejected unauthenticated requests.
- Host and guest HTTPS requests to the same address-checking endpoint returned
  the same public egress address. This verifies those requests only; it is not a
  universal routing or VPN kill-switch test.
- No shared host filesystems were mounted in the guest.
- A full stop/start reached ready in 9.3 seconds. DNS, HTTPS, unauthenticated API
  rejection, filesystem isolation, and matching host egress passed again.
- Cloud-init finished with no fatal errors and two generated-configuration
  deprecation warnings. These remain recorded; initialization failures must not
  be accepted merely because SSH works.

The disposable guest was then stopped and deleted through Lima; no evaluation VM
or Lima process remains running. The enrolled Multipass worker was left stopped.
This is a promising networking result, not completed NodeHarbor integration or
Kubernetes qualification. No host VPN settings or connections were changed.

## Delivery sequence

1. **Complete the native feasibility check.** The isolated guest boot, restart,
   connectivity, shutdown, and cleanup checks passed on the affected Apple Silicon
   Mac. Next validate the real application resource and state paths, including
   spaces and realistic instance-name lengths. Do not introduce temporary-path
   redirects or symlink aliases. Do not manipulate the host VPN to simulate
   network failures.

2. **Add a macOS VM adapter using TDD.** Define behavior for preparation, progress,
   cancellation, ownership, resource changes, guest execution, shutdown, and
   recovery before implementation. Use native Lima operations behind the VM
   interface; do not translate Multipass command strings through wrapper scripts.
   Persist the provider and instance identity in ownership bookkeeping. Preserve
   the current adapters on Windows and Linux, and never operate on unrelated VMs.

3. **Deliver it inside the application.** Bundle the required, verified Lima
   components and license notices into the macOS app using normal application
   resources. The user should not need another manual installer. Resolve the
   bundled runtime through the application layout and fail clearly if it is
   missing or damaged. Test both macOS architectures, retain the existing Windows
   and Linux packaging checks, and publish only installable release assets.

4. **Migrate through NodeHarbor's lifecycle.** Preserve enrollment, sharing policy,
   resource limits, and live activity logs. Make replacement of an existing worker
   explicit in the application. Verify ownership and stop the previous worker
   before starting its replacement. Account for retained disks and keep running
   resource use within the owner's budget. Configured workers must use the
   existing access-reset and drain flows; no manual credential or database edits.

5. **Prove an actual contributed worker.** Prepare the worker through the installed
   application, connect its guest to the existing private network, and verify
   Kubernetes registration. Check pod DNS, private API reachability, and MTU from
   that worker. Observe the controller's CI qualification period, run a real CI
   job on it, and confirm its node in Headlamp. Test pause, stop, restart, and
   recovery without modifying the host VPN. Service eligibility still requires
   its full configured observation window; do not shorten qualification to pass.

## Completion criteria

Run `make check` after changes and pass the native release gates. A fix is ready
for the affected Mac only when the installed app completes preparation, the
cluster sees a qualified node, a real job succeeds there, and stop/recovery work.
Report untested platforms and remaining qualification separately. Keep production
networking, installer delivery, and lifecycle evidence distinct from unit tests.

## Primary references

- [Lima user-v2 networking](https://lima-vm.io/docs/config/network/user-v2/)
- [Lima's native macOS VM driver](https://lima-vm.io/docs/config/vmtype/vz/)
- [Lima installation and distribution](https://lima-vm.io/docs/installation/)
- [Multipass startup troubleshooting](https://canonical.com/multipass/docs/latest/how-to-guides/troubleshoot/troubleshoot-launch-start-issues/)
