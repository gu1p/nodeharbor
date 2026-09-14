# Reproducible acceptance

NodeHarbor release validation does not require access to an existing private
deployment. From a clean checkout with the repository's Rust toolchain and
Python, run:

```sh
make acceptance
# Windows, or to select the evidence location:
python scripts/acceptance.py --output .local/acceptance-report.json
# While editing (publication rejects this development evidence):
python scripts/acceptance.py --allow-dirty
python scripts/acceptance.py --list
```

The command creates private, temporary SQLite databases, random credentials,
and loopback HTTP listeners. It uses production controller handlers, agent
storage logic, infrastructure HTTP clients, and qualification decisions.
Kubernetes, NetBird, VM execution, and observation time are explicit test
doubles in test targets. There is no production switch that enables them.
No existing deployment, application configuration, VPN, or routing is changed.
Listeners are stopped and temporary state is removed when each scenario ends.
The first run may download the pinned Cargo dependencies.

| Scenario | What it reproduces |
| --- | --- |
| Enrollment and authentication | One-use codes, all supported platform identities, invalid credentials, owner-bound bootstrap, administrator separation |
| Preparation failure and recovery | Infrastructure failure, unavailability across restart, retry, and retained owner pause |
| Qualification and owner controls | A complete simulated ten-minute window through the production reconciler, rejection before that window, pause, and failed independent probes |
| Missing storage and restart | Lost eligibility, persisted state, stale generations, fresh qualification, actual agent storage/recovery contracts |
| Revocation and re-enrollment | Immediate credential rejection, durable failed cleanup, retry, and fresh replacement identity |
| Infrastructure HTTP contracts | Real authenticated HTTP clients against Kubernetes/NetBird simulators, one-use grants, ownership, network evidence, and protected placement |

Every required scenario must execute tests and pass. Ignored tests and empty
test selections fail the command. A failed run produces no success report;
an existing output file is never overwritten. Reports identify the full source
commit, version, dirty-source status, test counts, and simulated infrastructure.
They do not claim that a physical VM joined a live cluster or ran a Kubernetes job.

## Native Android qualification

Build the signed APK and upgrade baseline with the persistent signing identity,
then pass the scenario report and explicitly selected authorized test devices:

```sh
python scripts/build_android.py VERSION COMMIT --release --upgrade-pair
python scripts/qualify_android.py dist VERSION COMMIT \
  --acceptance-report .local/acceptance-report.json \
  --serial PHONE --serial API36_EMULATOR --serial API37_EMULATOR
```

API 33, 36 and 37 ARM64 coverage and a stock physical phone remain required.
The application and instrumentation must share a certificate across both signed
versions. Upgrade, private-data persistence, native runtime, owner-control and
VPN-preservation checks execute on those devices. Approve normal Android install
prompts when required. Device serials belong in local configuration, never Git.

For the trusted-main runner, set `NODEHARBOR_ANDROID_DEVICES_FILE` to a private
JSON file containing only `{"devices": ["selected-device", "selected-emulator"]}`
with the full required API coverage. CI supplies the exact-commit scenario
report. No controller URL, enrollment code, kubeconfig, or deployment credentials
are required. Publication still waits for every desktop, controller, signing,
scenario, and native Android gate.

Optional additional deployment testing is available through
`--external-config PATH`. That mode retains the existing real enrollment,
normal qualification window, actual ARM64 Kubernetes workload, and owner-stop
checks. Its private configuration is described in `android.md`.
