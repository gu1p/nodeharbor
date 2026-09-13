# Android contribution contracts

One ARM64 APK supports Android 13 and newer through supported public APIs.
It contributes to the same fleet as desktop. Installing it never enables sharing.

## Owner-visible behavior

- Native, accessible navigation exposes Your phone, Sharing rules, Fleet and
  Connection. Status and failures are readable without relying on color.
- Continuous sharing enables opening, background, boot-after-unlock and screen-off
  execution preferences. Battery and metered use remain off. Every preference is
  separately adjustable. Permission requests never masquerade as granted settings.
- Closing the screen does not stop an explicitly enabled background worker.
  Pause, Stop, force-stop, revoked enrollment and expired owner leases stop work.
- Preparation shows bounded, redacted progress. Insufficient resources, blocked
  networking, unavailable runtimes and unsupported configurations explain why
  sharing cannot start. A failed or untested runtime cannot report ready.
- Resource replacement drains and resets owned access before deletion. The owner
  confirms destructive actions. Updates retain identity and policy but pause work.
- After draining, show accessible “Stopping” until guest poweroff and the original
  isolated process's Binder death are both observed. The whole graceful operation,
  including command-channel contention, has at most 120 seconds and cannot extend
  the original drain deadline. Repeated stops join the same operation.
- Expiry, critical resources and service destruction force teardown immediately,
  independently of startup and controller requests. Allow five seconds to confirm
  death. Show “Worker force-stopped” or “Shutdown unconfirmed”; an acknowledgement
  or lost binding alone cannot confirm termination. Unconfirmed workers retain
  their storage and block restart and replacement.
- Retain an already owner-enabled wake lock through bounded stopping, then release
  it. Explicit owner stops survive reopening and service recreation. Late responses
  from an old session cannot change the new session's state.
- Refresh the owned seed atomically while stopped. Its guest-control revision must
  appear in instance identity and status and match before readiness. Upgrading the
  seed preserves disk contents and enrollment.
- The guest owner channel starts after the platform services needed for normal
  poweroff. Lease monitoring starts after that channel can accept renewals, without
  creating a boot-order cycle. Startup cancellation still uses bounded teardown.

## Boundaries and acceptance

QEMU TCG is not trusted to isolate guest code. The emulator and packet processing
must run in an Android isolated process. It receives only owned disk descriptors
and restricted IPC capabilities; it cannot read enrollment credentials or phone
files. Host network access uses ordinary app routing and preserves VPN policy.

Test accessible UI contracts, then unit decisions, then service/controller/runtime
integration before implementation. Record each failing check and run make check
after each change. Native Android results must identify the APK digest and source.
Emulator tests do not establish sustained physical-device contribution.

A supported release requires the actual packaged runtime on a physical phone,
normal CI qualification and a successful real ARM64 job. Service qualification
retains its full observation window. Public test records contain no credentials,
device identifiers or private infrastructure details.
