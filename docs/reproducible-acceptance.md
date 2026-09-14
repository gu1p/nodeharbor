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

## Optional native Lima storage proof

The public native fixture can create its own unenrolled Lima VM and temporary
Kubernetes cluster. It needs a supported Linux or macOS virtualization host, the
pinned Lima runtime, and two writable folders on distinct supported volumes.
It creates owned disk images inside those folders. It does not format host devices
or use a private deployment. Allow at least 25 GiB free on the primary volume and
40 GiB on the secondary volume for the test and its retained data.

```sh
export NODEHARBOR_TEST_LIMA=/absolute/path/to/verified/limactl
export NODEHARBOR_TEST_STORAGE_PRIMARY=/folder/on/first-volume
export NODEHARBOR_TEST_STORAGE_SECONDARY=/folder/on/second-volume
NODEHARBOR_NATIVE_KEEP=1 cargo test --locked -p nodeharbor-agent \
  --test native_lima_storage \
  native_plain_vm_uses_a_persistent_ext4_pool_across_host_volumes \
  -- --ignored --exact --nocapture
```

The `NATIVE_STORAGE_FIXTURE` output reports the private fixture's `home` directory.
Keep that output local. Pass its exact `home` value to the workload proof:

```sh
NODEHARBOR_NATIVE_KUBERNETES=1 python tests/native_storage_kubernetes.py \
  --lima "$NODEHARBOR_TEST_LIMA" --home /the/reported/lima/home
```

The proof validates the guest's ownership receipt, installs pinned K3s in that
guest, writes a file larger than any member disk, checks Kubernetes capacity and
usage, restarts the VM, and verifies the file's checksum and filesystem identity.
It waits for the current guest boot and a responding kubelet after restart.
Verification timeouts scale with file size for slower drives. A failed check
produces no passing evidence. The temporary cluster is stopped on exit; the owned
VM and its files remain for inspection. Stop that specific fixture afterward:

```sh
LIMA_HOME=/the/reported/lima/home "$NODEHARBOR_TEST_LIMA" stop worker
```

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

To repeat the same candidate on disposable test devices, add
`--reset-test-installation`. Only when a newer version prevents baseline installation,
the tool verifies the installed signing certificate, stops the app through owner
controls, and removes that app and its instrumentation before installing the baseline.
This explicitly deletes NodeHarbor's test data; it preserves other apps and phone
settings. The dedicated CI runner uses this option. Normal runs preserve installed
app data and refuse an incompatible update.

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
