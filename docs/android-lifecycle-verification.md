# Android VM lifecycle work — 2026-09-13

The milestone is **incomplete**. The owner requested testing only on the connected
physical Android phone because the development host cannot spare memory for
emulators. No further emulator launches are authorized for this work.

The development APK is ready. Its unit tests, lint, builds, 38 physical component
and UI contracts, and real existing-disk upgrade passed. In response to the
owner's concern about elapsed time, extended testing was stopped for handover.
The current APK completed two of the required five consecutive graceful cycles;
the third boot was intentionally interrupted. This is an incomplete acceptance
run, not a passed five-cycle test.

## Implemented changes

- A per-session shutdown coordinator distinguishes graceful shutdown, forced
  teardown, unexpected exit and unconfirmed termination. Graceful shutdown uses
  the smaller of 120 seconds and the original drain time remaining; forcing has
  five seconds to confirm termination. Repeated requests cannot extend either
  allowance or repeat teardown.
- The supervisor drains before requesting poweroff. Deadline expiry, expired
  controller authority, critical resources and service destruction force teardown
  independently of guest startup and controller requests. Guest status and lease
  scheduling stop when shutdown starts; session epochs reject stale responses.
- Command deadlines include waiting for the private channel lock. Binding loss
  remains distinct from the original Binder's death. Graceful classification needs
  a recognized kernel poweroff message and process death. Unconfirmed sessions
  retain their disk reservation and block restart, seed writes and replacement.
- Cleanup releases the broker, descriptors, bindings and executors. An already
  owner-enabled wake lock remains held during bounded stopping. Accessible states
  distinguish stopping, forced stopping and unconfirmed termination. Explicit
  owner stops remain persisted across reopening and service recreation.
- Owned seeds refresh atomically, include control revision `4` in instance identity
  and status, and restart an existing control service after its code changes.
  Readiness requires matching code and active systemd-unit revisions. A legacy
  unit can continue serving status and owner leases without advertising readiness;
  configured-worker status also defers service queries until the new unit is active.
  Owner lease renewal uses the same
  atomic watchdog lease writer directly rather than spawning another interpreter.
- The control service waits for D-Bus and logind before accepting owner commands.
  Its watchdog timer waits for control readiness without the implicit early
  `timers.target` dependency. Re-enabling the owned units updates existing disks'
  previous enablement links. Normal `systemctl poweroff` and its ten-second command
  timeout are unchanged, as are Android's 120-second graceful and five-second
  forced-confirmation limits.

Android documents [Binder death notification](https://developer.android.com/reference/android/os/IBinder.DeathRecipient)
separately from [service connection callbacks](https://developer.android.com/reference/android/content/ServiceConnection).
The implementation retains that distinction after unbinding.

## Failures recorded before fixes

The revision-2 disk upgrade exposed a remaining production failure in revision 3:
the refreshed Python file could report the new revision while its process still
used the legacy systemd service definition. That upgrade boot answered, preserved
the marker, then exceeded the 120-second graceful deadline. Forced process death
was confirmed 283 ms later. The failing device assertion is retained in
`.local/android-device/d5fc8812-bff5-4f0f-b68e-f68b2cfbd11e.log`.
Readiness must therefore identify both the control code and the active service
definition. A process launched by an older unit must continue serving owner
leases without advertising the expected control revision until the upgraded
unit, including its poweroff dependencies, has actually been activated.

The revision-4 correction passed its unit and physical seed regressions, then
passed the same real disk upgrade. The legacy boot answered in 535.792 seconds
and stopped gracefully in 79.585 seconds. The upgraded boot withheld readiness
until the new unit was active, answered in 599.545 seconds and stopped gracefully
in 89.025 seconds. Its flushed marker, ownership receipt and VPN fingerprint
were preserved. This test uses an unenrolled guest; actual fleet enrollment
preservation still needs the authorized test-fleet configuration.

Fault-injection seeds must receive a new cloud-init identity when a test reapplies
configuration, including a return to normal behavior. The fault matrix exposed a
test-fixture defect: after the unresponsive-channel stop, cloud-init completed but
the reused normal identity retained the previous fault script. The saved disk
marker remained intact. A failing runtime assertion is retained in
`.local/android-lifecycle-reused-fixture-runtime-red.log`. This is not a successful
recovery result. A focused seed contract then failed with two unique identities
instead of three; it passes after assigning each test seed application a fresh
identity. Recovery resumed from the checkpointed disk and passed. That fixture
correction did not change the previous revision-3 application APK.

Private, ignored `.local/android-lifecycle-*-red-*.log` files retain the failing
checks. They cover accessible restart blocking, shutdown decisions, command lock
contention, missing revision reporting, atomic seed refresh, original-Binder death
tracking, storage exclusion, lifecycle display recovery, bounded diagnostics,
existing-service code adoption, direct lease renewal and authority expiry.

The previous second-shutdown timeout is preserved in `android-verification.md`
and its original private log; the 120-second limit was not raised. A new diagnostic
two-boot run confirmed its first poweroff, then failed on the second boot's lease
command before requesting shutdown. The extra Python process used for that lease
had a ten-second timeout. That observed defect has a regression test and a direct
atomic renewal implementation.

After the owner authorized closing cached phone apps, the physical API 33 phone
reproduced the second-shutdown failure with control revision 2. Boot one answered
in 556.907 seconds and stopped gracefully in 81.344 seconds. Boot two answered in
325.777 seconds and acknowledged shutdown immediately, but its private console
recorded `systemctl poweroff` timing out after ten seconds while normal services
were still starting. Android forced teardown at the unchanged 120-second deadline;
process death followed 0.283 seconds later. The run failed after 1109.632 seconds,
and the disk marker had survived the restart. The VPN fingerprint was unchanged.

The failed runtime assertion is retained privately in
`.local/android-device/73192d40-0b09-4ae7-901a-8cb3d1f3baf5.log`.
Before changing startup ordering, a new physical seed contract failed because
the owner channel did not wait for D-Bus, and a unit contract rejected revision 2.
Both regressions pass with the ordering change and revision 3. That previous
revision-3 APK passed five consecutive graceful cycles on the same stock API 33
phone and the same disk. Each result required kernel poweroff and original Binder death;
none used forced fallback. The previously flushed marker survived every restart,
and the phone VPN fingerprint was unchanged.

| Cycle | Boot to owner response (seconds) | Graceful shutdown (seconds) |
| --- | ---: | ---: |
| 1 | 550.852 | 75.967 |
| 2 | 385.916 | 116.596 |
| 3 | 384.546 | 103.351 |
| 4 | 401.107 | 107.534 |
| 5 | 390.263 | 107.319 |

The guest still logged a command-client timeout during some successful poweroffs;
acknowledgement or command completion is not used as termination evidence. The
longest observed graceful shutdown was close to the unchanged 120-second limit.
These results apply only to the previous revision-3 APK. Repeated lifecycle
acceptance remains unfinished for the delivered revision-4 APK. The historical
failure/recovery results and outstanding acceptance are recorded below.

Systemd 255's [poweroff implementation](https://github.com/systemd/systemd/blob/v255/src/systemctl/systemctl-start-special.c)
first calls logind. Its [timer defaults](https://github.com/systemd/systemd/blob/v255/man/systemd.timer.xml)
order ordinary timers before `timers.target`. The changed ordering addresses the
observed early command failure without introducing a direct or forced guest
poweroff path.

## Development artifact and checks

The current development APK is
`dist/nodeharbor-android-lifecycle-development.apk`, version
`0.1.0-lifecycle-development.2`, control revision `4`, SHA-256
`b9093a6d2119a7d5deacb5e098b98820adccba3cc1aa05aede5c6bcb733fe114`.
It identifies base commit `a21633c265a54178f2419d3cdd2b580af761dfd5`
and a dirty working tree. That base commit is not a tested release source commit.
The build-time source-file digest manifest and sanitized evidence are in
`dist/android-lifecycle-source-files.json` and
`dist/android-lifecycle-development-evidence.json`.

- `make check` passed after the implementation changes.
- Android debug and release unit tests: 51 passed in each variant. Both variants'
  lint and APK builds passed. Release output is unsigned. AGP's documented
  `-Pandroid.onlyEnableUnitTestForTheTestedBuildType=false` option enables both
  unit-test variants without changing signing configuration.
- The exact current APK passed 38 stock Android 13 component and UI contracts,
  including isolation, network, storage, private channel, owner controls, wake
  ownership and background service behavior. The VPN fingerprint was unchanged.
- An earlier candidate APK completed one graceful cycle on each emulator: API 36
  shutdown took 46.074 seconds, API 37 took 46.436 seconds. Both entered boot two;
  the runs were interrupted before completing five cycles. These results neither
  qualify the emulators nor apply to the subsequently updated APK above.

## Outstanding physical acceptance

The delivered revision-4 APK completed these consecutive cycles on the same disk:

| Cycle | Boot to owner response (seconds) | Graceful shutdown (seconds) |
| --- | ---: | ---: |
| 1 | 636.188 | 82.577 |
| 2 | 413.515 | 112.050 |

Both required guest kernel poweroff and original Binder death, with no forced
fallback. The flushed marker and complete ownership receipt survived the restart.
Boot three was interrupted for handover using Android's activity-manager crash
command against the test owner. The original isolated process was confirmed gone
within 269 ms. This administrative stop is not a graceful acceptance result or a
completed owner-death recovery test. Only the validated temporary 15 GiB test disk
was removed after confirmed termination. App settings and production storage
were preserved, and the VPN fingerprint remained unchanged. The original
instrumentation run is retained as interrupted with a failing exit status.

Closing 26 cached user apps raised available phone RAM from approximately
1.62 GiB to 2.64 GiB; it reported 2863.6 MiB before the revision 3 runtime retest.
No app data was removed and the active phone VPN was preserved. The 2.5 GiB startup
threshold is an existing **application policy**: 2 GiB VM allocation plus 512 MiB
phone reserve, enforced by `phoneDecision`. It is not a measured minimum for this
workload. The supplied plan explicitly required preserving it; this work has not
lowered the allocation, bypassed the guard, or changed the phone's settings to
obtain a passing result.

Owner-process termination recovery passed on the previous revision-3 APK. Android's
activity-manager crash command targeted only the owner PID during a recovery boot.
The original isolated VM disappeared within 211 ms. A subsequent instrumentation
run reopened the same disk, verified its flushed marker and unchanged ownership
receipt, and completed a graceful shutdown in 100.204 seconds. The VPN fingerprint
was unchanged. The interrupted fault-sequence run that supplied this fixture does
not count as a completed fault matrix.

The physical failure/recovery cases completed across checkpointed runs on the
previous revision-3 APK. The original combined run was interrupted to correct the
reused test-seed identity; it is retained as an interrupted run, not a passed
single-run matrix. Its disk and ownership receipt were preserved. The resumed
run completed recovery, guest panic, authority expiry and subsequent reboots.

| Trigger | Forced teardown to process-death confirmation (ms) | Recovery graceful shutdown (seconds) |
| --- | ---: | ---: |
| Stop during boot | 23 | 109.754 |
| Stalled shutdown command | 272 | 97.711 |
| Guest kernel panic | 274 | 104.149 |
| Expired owner authority | 293 | 109.898 |
| Owner-process crash | 211 | 100.204 |

The stalled command took 3.276 seconds overall, including its command timeout.
Its recovery included the checkpoint and additional owner crash used for the
fixture correction. All completed recovery checks preserved the flushed marker
and entire ownership receipt. VPN fingerprints remained unchanged. The test-only
continuation APK is `dist/android-lifecycle-recovery-tests.apk`, SHA-256
`751cbc18dc42239520a1c9a019bbecc86a8c13d74a35e8c87d968c9ae3b3f75c`;
its source digest manifest is delivered separately from the original app build.

The revision-4 existing-disk upgrade passed as recorded above. Five consecutive
graceful cycles remain incomplete; final-APK fault and owner-death recovery
revalidation have not run. No VM or extended test remains running after handover.
Prior revision-3 results do not qualify this changed APK. One hour of screen-off operation remains
outstanding. The phone installation is unenrolled; running the background check
through normal owner controls needs a private authorized test-fleet configuration.
Instrumentation contracts now cover five cycles with a flushed guest disk marker,
stop during boot, an unresponsive guest, a guest panic, expired owner authority,
revision 2 disk upgrades, and owner-process termination followed by recovery;
the completed cases are detailed above. The authority-expiry fixture
uses the production lease clock and then requests forced VM teardown; it does not
claim to exercise a real fleet HTTP failure. Component tests alone do not establish
the behavior of these cases under full VM load.

No publication, production signing, fleet qualification, authentication bypass,
host VPN change or routing exception was performed.
