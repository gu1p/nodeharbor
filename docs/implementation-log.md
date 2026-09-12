# Implementation and verification log

## Initial contracts

- Repository created at https://github.com/gu1p/nodeharbor (public).
- UI behavioral contracts were written before implementation. `npm test` failed
  because `App` did not exist.
- Policy unit contracts were written before implementation. `cargo test -p
  nodeharbor-core --test policy_contract` failed on missing policy APIs.
- Enrollment API integration and worker lifecycle contracts were written before
  their implementations.
- `make check` failed on the missing release implementation, as expected in Red.

## Delivery checklist

- [ ] Desktop controls and native packaging for five supported targets.
- [ ] Background agent, managed VM lifecycle, and device enrollment.
- [ ] Persistent controller, authentication, fleet dashboard and eligibility.
- [ ] NetBird and Kubernetes adapters, observed health and draining.
- [ ] Automatic complete releases for every passing main push.
- [ ] One-line installers and upgrade failure coverage.
- [ ] Infra deployment with monitoring, identity, networking and CI pools.
- [ ] Native installation and real contributed-worker acceptance evidence.
- [ ] Production qualification window and recovery demonstration.

GitHub-hosted builders and pilot packages without OS distribution certificates
are approved. No additional Sikalio cloud machines may be created. Do not remove
older Tempo cleanup resources as part of this task.

## Local verification, 12 September 2026

`make check` passes with the desktop crate included: Python behavioral tests,
accessible React contracts, TypeScript compilation, Vite production build,
Rust unit and integration tests, formatting, and Clippy with warnings denied.
The native Tauri app also compiles on macOS ARM64.

Implemented and covered locally:

- Resource, battery, idle, and weekly schedule rules; sharing defaults off.
- Persistent device settings, credential redaction, ownership-checked VM commands,
  a guest lease watchdog, and incomplete-preparation protection.
- Native UI commands, tray menu, background preference, autostart integration,
  and drain-on-quit behavior.
- Agent CLI status and update preparation, tested as actual subprocesses.
- One-use enrollment, hashed device credentials, revocation, authenticated fleet
  reads, and trusted-gateway identity checks with request-origin protection.
- Kubernetes and NetBird HTTP adapters tested against local HTTP fixtures:
  separate credentials, expiring bootstrap tokens, device-specific network groups,
  one-use setup keys, cordoning, eviction, and removal.
- Release metadata validation, version stamping, ordering of the latest release,
  shell platform detection, and preservation on failed checksum verification.

Remaining work includes independent live health reconciliation, complete native
packaging and release automation, the Windows installer, deployed infrastructure,
real contributed CI execution, and the production qualification/recovery evidence.
No live Kubernetes worker has been enrolled by this project yet.
