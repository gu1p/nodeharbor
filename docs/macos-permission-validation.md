# macOS temporary-folder permissions

## Observed failure

Lima 2.2.0 creates its boot ISO workspace with `os.MkdirTemp("", "diskfs_iso")`,
which uses the inherited temporary directory. A host with an external `TMPDIR`
reported `operation not permitted` during startup. The macOS privacy log
identified denied Removable Volumes access attributed to NodeHarbor.

The installed app also failed `codesign --verify --strict` with
`code has no resources but signature indicates they must be present`.
The bundled Lima executable passed verification and retained its virtualization
and network entitlements. These are separate findings: repairing the app signature
does not grant the owner's consent.

## Changes

- Tauri signs pilot app bundles with the supported ad hoc identity `-` before
  packaging. The bundle includes a removable-volume usage explanation. The
  verified upstream Lima distribution and its embedded signature remain intact.
- Only failed macOS Lima `start`/`create` operations with the host-agent
  `mkdir .../diskfs_iso<digits>` permission-denial pattern receive recovery
  guidance. Other operations, platforms, and errors keep their original output.
- The guidance precedes bounded, redacted diagnostics so that the activity and
  VM error limits cannot hide it. Live activity retains the underlying error
  lines through the existing redaction path.
- Worker failures are accessible alerts. Replacement keeps its existing alert
  and owner controls. No new API or configuration format is introduced.
- Both installer and update-archive smoke checks verify the app signature,
  identity, version, permission description, required executables, Lima
  signature and entitlements, runtime hashes, and executable source/version.

`TMPDIR`, directory permissions, VM ownership, and host VPN settings are preserved.
The owner grants access through macOS. No privilege elevation, alternate temporary
directory, TCC database modification, or automatic permission reset is used.

## TDD evidence, 2026-09-14

Behavior/UI contracts were added first, then unit contracts, then integration
contracts, followed by implementation. `make check` ran after each change;
targeted checks exposed later stages while earlier contracts were still red.

| Failing check recorded before correction | Correction |
| --- | --- |
| Pilot configuration had no signing identity | Enable Tauri's ad hoc bundle signing |
| Worker permission error had no accessible alert | Announce worker errors while preserving controls |
| Startup diagnostic unit tests could not find the helper | Add the narrow macOS error classifier |
| Bundle-validation helper was absent; package integration observed zero calls | Validate the app in both package forms |
| Process integration received no Removable Volumes guidance | Map the runtime failure before activity and VM truncation |
| Replacement UI regression found two alerts | Retain its existing alert instead of adding a duplicate |
| Existing update test intermittently exhausted its polling deadline | Poll the in-memory error log, then assert the snapshot once; avoid rescanning host disks on every poll |

Local logs are under `.local/check-macos-permissions-*.log` and
`.local/check-lima-permissions-*.log`; host-specific evidence is not committed.

## Results on macOS arm64

- `make check` passed: Python contracts, 81 UI tests, workspace Rust tests,
  formatting, and Clippy with warnings denied. Platform-specific skips and
  opt-in native tests are not counted as native acceptance.
- The existing update/recreation test timed out intermittently, including after
  the native build finished, while its isolated run passed. Its polling loop
  now waits on in-memory activity and checks the full error snapshot once.
  The original timeout and update behavior assertions are retained.
- The rebuilt development app passed signature, identity, permission-description,
  executable provenance, and unchanged Lima hash/entitlement verification.
  The native DMG, copied app, and tar archive passed the same package smoke checks.
- The isolated real Lima test passed creation, ownership, budget, filesystem
  isolation, HTTPS, guest update, resize, and restart checks. Its VM was stopped
  and removed. The inherited external `TMPDIR` and VPN policy were kept.
- Artifacts in `.local/macos-permission-acceptance` are local development builds
  reporting `0.1.0 (development)`, not release packages. No release was published.

Interactive desktop deny/grant/relaunch acceptance remains unverified. Native
Intel macOS, Linux, and Windows were not exercised in this session.

## Native acceptance boundaries

Automated native signature fixtures exercise unsealed rejection, valid ad hoc
signing, copying a sealed app, and rejection after resource tampering. A synthetic
Lima subprocess checks denial, retained ownership, simulated grant/retry, secret
redaction, and unchanged `TMPDIR`, including the streaming path. A simulated grant
does not establish that the macOS consent flow works.

For desktop consent acceptance, use isolated worker data through
`NODEHARBOR_CONFIG_DIR` and an owner-approved test session. Keep the existing VPN
and external temporary directory. Deny the normal macOS removable-volume prompt,
check the readable error and stopped worker, grant access in System Settings,
fully quit/relaunch, then verify worker creation, ownership, stop, and restart.
Check that the installed or updated bundle still verifies. This interactive
sequence must be recorded separately from command-line VM tests and repeated on
native Apple Silicon and Intel hosts before claiming both platforms qualified.

Pilot ad hoc signatures do not guarantee persistent privacy permissions across
updates. Full Disk Access is a broader manual alternative if the narrower entry
is unavailable. Developer ID signing and notarization remain outside this fix.

## Sources

- [Pinned Lima ISO creation](https://github.com/lima-vm/lima/blob/v2.2.0/pkg/iso9660util/iso9660util.go)
- [Tauri ad hoc signing](https://v2.tauri.app/distribute/sign/macos/)
- [Apple removable-volume usage description](https://developer.apple.com/documentation/bundleresources/information-property-list/nsremovablevolumesusagedescription)
- [Apple privacy settings](https://support.apple.com/guide/mac-help/change-privacy-security-settings-on-mac-mchl211c911f/mac)
- [Apple guidance on ad hoc signing and TCC](https://developer.apple.com/forums/thread/125438)
