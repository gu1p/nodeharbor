#!/usr/bin/env python3
"""Opt-in Kubernetes proof in a retained native_lima_storage test fixture.

Run with NODEHARBOR_NATIVE_KUBERNETES=1 and explicit --lima and --home.
Only that receipt-owned nh724-vm fixture is changed or restarted. No fleet
enrollment is created. Its temporary cluster is stopped when the test exits;
the VM and its data remain available for inspection and fixture cleanup.
"""
import argparse
from decimal import Decimal
import json
import os
from pathlib import Path
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
GIB = 1024 ** 3
IMAGE = 'docker.io/library/busybox:1.36.1@sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662'
NODE = 'nodeharbor-storage-proof'
POD = 'storage-cross-disk'
NAMESPACE = 'nodeharbor-storage-proof'
SERVICE = 'nodeharbor-storage-proof.service'
KUBECONFIG = '/etc/nodeharbor/storage-proof/kubeconfig.yaml'


def progress(message):
    print('Native storage proof: ' + message, flush=True)


def quantity(value):
    match = re.fullmatch(r'([0-9.]+)(Ki|Mi|Gi|Ti|K|M|G|T)?', str(value))
    if not match:raise ValueError('Unsupported Kubernetes capacity quantity')
    suffix = match[2] or ''
    multiplier = {'': 1, 'Ki': 1024, 'Mi': 1024 ** 2, 'Gi': 1024 ** 3, 'Ti': 1024 ** 4,
                  'K': 1000, 'M': 1000 ** 2, 'G': 1000 ** 3, 'T': 1000 ** 4}[suffix]
    return int(Decimal(match[1]) * multiplier)


class Fixture:
    def __init__(self, binary, home):
        self.binary = Path(binary).resolve(); self.home = Path(home).resolve()
        if self.home.name != 'lima' or not self.home.parent.name.startswith('nh724-vm-'):
            raise ValueError('Use only a retained nh724-vm native test fixture')
        self.receipt = json.loads((self.home.parent / 'worker.receipt.json').read_text())
        if self.receipt.get('version') != 2 or self.receipt.get('provider') != 'lima' or self.receipt.get('name') != 'worker':
            raise ValueError('The selected test VM has no matching Lima ownership receipt')
        self.owner = self.receipt['deviceId']
        self.environment = {**os.environ, 'LIMA_HOME': str(self.home), 'SSH': '/usr/bin/ssh',
                            'PATH': str(self.binary.parent) + ':/usr/bin:/bin:/usr/sbin:/sbin'}

    def lima(self, *arguments, input=None, timeout=120):
        result = subprocess.run([str(self.binary), '--tty=false', *arguments], env=self.environment,
                                input=input, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
        if result.returncode:
            raise RuntimeError(f'Lima command failed ({arguments[0]}): {result.stderr[-4000:]} {result.stdout[-2000:]}')
        return result.stdout

    def shell(self, *command, **kwargs):
        return self.lima('shell', '--workdir=/', 'worker', 'sudo', *command, **kwargs)

    def python(self, source, payload=None, **kwargs):
        return self.shell('python3', '-c', source, input=json.dumps(payload) if payload is not None else None, **kwargs)

    def verify_owner(self):
        if self.shell('cat', '/etc/nodeharbor/device-id').strip() != self.owner:
            raise ValueError('The running guest does not match its ownership receipt')

    def pool(self):
        return json.loads(self.shell('python3', '/usr/local/lib/nodeharbor/storage_pool.py', 'check'))

    def kubectl(self, *arguments, **kwargs):
        return self.shell('/usr/local/bin/k3s', 'kubectl', '--kubeconfig', KUBECONFIG, *arguments, **kwargs)


PREPARE = r"""
import json,pathlib,platform,sys
sys.path.insert(0,'/usr/local/lib/nodeharbor')
from configure_worker import cached_download,write_bytes,write
payload=json.load(sys.stdin)
root=pathlib.Path('/etc/nodeharbor')
assert (root/'device-id').read_text().strip()==payload['owner']
assert not pathlib.Path('/etc/rancher/k3s/config.yaml').exists(), 'An enrolled worker configuration must not be replaced by this test'
proof=root/'storage-proof'; marker=proof/'owner'
if marker.exists():assert marker.read_text().strip()==payload['owner']
write(str(marker),payload['owner'])
runtime=payload['runtime']; name='k3s-arm64' if platform.machine()=='aarch64' else 'k3s'
asset=next(asset for asset in runtime['assets'] if asset['name']==name)
data=cached_download('https://github.com/k3s-io/k3s/releases/download/'+runtime['k3sVersion']+'/'+name,
                     asset['sha256'],pathlib.Path('/var/cache/nodeharbor/runtime'))
write_bytes('/usr/local/bin/k3s',data,0o755)
pool='/var/lib/nodeharbor/storage'
configuration={'data-dir':pool+'/k3s','node-name':payload['node'],'write-kubeconfig':str(proof/'kubeconfig.yaml'),
               'disable':['traefik','servicelb','local-storage','metrics-server'],
               'resolv-conf':'/run/systemd/resolve/resolv.conf',
               'kubelet-arg':['root-dir='+pool+'/kubelet','system-reserved=cpu=100m,memory=256Mi',
                              'kube-reserved=cpu=150m,memory=256Mi',
                              'eviction-hard=memory.available<256Mi,nodefs.available<10%,imagefs.available<15%']}
write(str(proof/'server.json'),json.dumps(configuration))
write(pool+'/k3s/agent/etc/kubelet.conf.d/10-nodeharbor.conf',json.dumps({
    'apiVersion':'kubelet.config.k8s.io/v1beta1','kind':'KubeletConfiguration',
    'nodeStatusReportFrequency':'30s','podLogsDir':pool+'/pods'}),0o644)
unit='''[Unit]
Description=Isolated NodeHarbor native storage test
Requires=nodeharbor-storage.service
After=network-online.target nodeharbor-storage.service
[Service]
Type=notify
TimeoutStartSec=300
ExecStart=/usr/local/bin/k3s server --config /etc/nodeharbor/storage-proof/server.json
Restart=on-failure
Delegate=yes
LimitNOFILE=1048576
TasksMax=infinity
[Install]
WantedBy=multi-user.target
'''
write('/etc/systemd/system/nodeharbor-storage-proof.service',unit,0o644)
print('Pinned K3s installed; isolated test configuration prepared')
"""


def wait_ready(fixture):
    boot = fixture.shell('cat', '/proc/sys/kernel/random/boot_id').strip()
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        try:
            value = json.loads(fixture.kubectl('get', 'node', NODE, '-o', 'json', timeout=15))
            current_boot = value['status'].get('nodeInfo', {}).get('bootID') == boot
            if current_boot and any(condition['type'] == 'Ready' and condition['status'] == 'True' for condition in value['status'].get('conditions', [])):
                # Ready is persisted across restarts; verify the current kubelet
                # can serve requests before using its Pod execution endpoint.
                if fixture.kubectl('get', '--raw', f'/api/v1/nodes/{NODE}/proxy/healthz', timeout=15).strip() == 'ok':
                    return value
        except (RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError, KeyError):pass
        time.sleep(3)
    raise RuntimeError('The isolated Kubernetes node did not become Ready')


def payload_size(fixture):
    # BusyBox wc streams stdin, adding another full disk read to this check.
    # The separate SHA-256 checks verify contents before and after VM restart.
    return int(fixture.kubectl('exec', '-n', NAMESPACE, POD, '--',
                              'stat', '-c', '%s', '/scratch/payload').strip())


def payload_timeout(size):
    # Allow a full verification read on a 16 MiB/s volume, plus command overhead.
    throughput = 16 * 1024 ** 2
    return max(180, (size + throughput - 1) // throughput + 60)


def workload_script(size):
    return ('set -eu; rm -f /scratch/ready; if [ -f /scratch/payload.sha256 ]; then sha256sum -c /scratch/payload.sha256; '
            f'else dd if=/dev/urandom of=/scratch/payload bs=4M count={size // (4 * 1024 ** 2)}; '
            'sync; sha256sum /scratch/payload > /scratch/payload.sha256; sync; fi; '
            'touch /scratch/ready; echo nodeharbor-cross-disk-ready; sleep 7200')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lima', required=True); parser.add_argument('--home', required=True)
    arguments = parser.parse_args()
    if os.environ.get('NODEHARBOR_NATIVE_KUBERNETES') != '1':
        raise SystemExit('Set NODEHARBOR_NATIVE_KUBERNETES=1 to opt into this isolated VM test')
    fixture = Fixture(arguments.lima, arguments.home); fixture.verify_owner()
    version = fixture.lima('--version').strip()
    pinned_lima = json.loads((ROOT / 'runtime/lima.json').read_text())['version']
    if not re.search(r'\b' + re.escape(pinned_lima) + r'\b', version):
        raise ValueError('The test requires the pinned Lima runtime')
    pool = fixture.pool()
    if pool['deviceId'] != fixture.owner or not pool['migrationComplete'] or len(pool['disks']) < 2:
        raise ValueError('The test requires a migrated pool with multiple owned disks')
    largest = max(disk['allocationBytes'] for disk in pool['disks'])
    total = sum(disk['allocationBytes'] for disk in pool['disks'])
    write_bytes = (largest // GIB + 1) * GIB
    verification_timeout = payload_timeout(write_bytes)
    existing_proof = fixture.python(
        "from pathlib import Path; p=Path('/etc/nodeharbor/storage-proof/owner'); print(p.read_text().strip() if p.exists() else '')").strip()
    if existing_proof and existing_proof != fixture.owner:raise ValueError('Existing native proof belongs to another owner')
    if not existing_proof and pool['availableBytes'] < write_bytes + 3 * GIB:
        raise ValueError('The fixture needs room for a cross-disk file and Kubernetes reserves')
    clock_skew = abs(time.time() - float(fixture.shell('date', '+%s').strip()))
    if clock_skew > 30:raise ValueError('The plain guest clock is not sufficiently aligned with its host')
    progress('Installing the locked K3s version in the isolated guest')
    runtime = json.loads((ROOT / 'runtime-lock.json').read_text())
    fixture.python(PREPARE, {'owner': fixture.owner, 'runtime': runtime, 'node': NODE}, timeout=180)
    started = False
    try:
        fixture.shell('systemctl', 'daemon-reload')
        started = True
        fixture.shell('systemctl', 'enable', '--now', SERVICE, timeout=330)
        node = wait_ready(fixture)
        capacity = quantity(node['status']['capacity']['ephemeral-storage'])
        allocatable = quantity(node['status']['allocatable']['ephemeral-storage'])
        if not largest < allocatable <= capacity <= total or abs(capacity - pool['capacityBytes']) > 4096:
            raise ValueError(f'Kubernetes does not advertise the real combined pool: capacity={capacity}, allocatable={allocatable}, filesystem={pool["capacityBytes"]}')
        progress(f'Kubernetes advertises {capacity} bytes capacity and {allocatable} bytes allocatable; writing {write_bytes} random bytes')
        script = workload_script(write_bytes)
        pod = {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': {'name': POD, 'namespace': NAMESPACE},
               'spec': {'restartPolicy': 'Always', 'containers': [{'name': 'write', 'image': IMAGE,
                        'command': ['sh', '-c', script],
                        'resources': {'requests': {'cpu': '100m', 'memory': '64Mi', 'ephemeral-storage': str(write_bytes + GIB)},
                                      'limits': {'cpu': '1', 'memory': '256Mi', 'ephemeral-storage': str(write_bytes + GIB)}},
                        'readinessProbe': {'exec': {'command': ['test', '-f', '/scratch/ready']}, 'periodSeconds': 3},
                        'volumeMounts': [{'name': 'scratch', 'mountPath': '/scratch'}]}],
                        'volumes': [{'name': 'scratch', 'emptyDir': {'sizeLimit': str(write_bytes + GIB)}}]}}
        fixture.kubectl('apply', '-f', '-', input=json.dumps(
            {'apiVersion': 'v1', 'kind': 'Namespace', 'metadata': {'name': NAMESPACE}}))
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            try:
                fixture.kubectl('get', 'serviceaccount', 'default', '-n', NAMESPACE, timeout=10)
                break
            except RuntimeError:time.sleep(2)
        else:raise RuntimeError('The isolated namespace default service account did not become available')
        fixture.kubectl('delete', 'pod', POD, '-n', NAMESPACE, '--ignore-not-found', '--wait=true')
        fixture.kubectl('apply', '-f', '-', input=json.dumps(pod))
        initial_timeout = 2 * verification_timeout + 180
        fixture.kubectl('wait', '--for=condition=Ready', f'pod/{POD}', '-n', NAMESPACE,
                        f'--timeout={initial_timeout}s', timeout=initial_timeout + 30)
        expected = fixture.kubectl('exec', '-n', NAMESPACE, POD, '--', 'cat', '/scratch/payload.sha256').split()[0]
        actual_size = payload_size(fixture)
        if actual_size != write_bytes:raise ValueError('The Pod did not write the complete cross-disk file')
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            summary = json.loads(fixture.kubectl('get', '--raw', f'/api/v1/nodes/{NODE}/proxy/stats/summary'))
            candidates = [item for item in summary.get('pods', []) if item['podRef']['name'] == POD]
            if candidates and candidates[0].get('ephemeral-storage', {}).get('usedBytes', 0) >= write_bytes:break
            time.sleep(5)
        else:raise ValueError('Kubelet did not account for the cross-disk emptyDir usage')
        if summary['node']['fs']['capacityBytes'] != capacity or summary['node']['runtime']['imageFs']['capacityBytes'] != capacity:
            raise ValueError('Node and container image storage are not using the same pooled filesystem')
        progress('Cross-disk file and accounting passed; restarting only this owned test VM')
        fixture.lima('stop', 'worker', timeout=180)
        fixture.lima('start', 'worker', '--timeout=5m', timeout=330)
        fixture.verify_owner(); after = fixture.pool()
        if after['filesystemUuid'] != pool['filesystemUuid']:raise ValueError('Pool identity changed after VM restart')
        node_after = wait_ready(fixture)
        restart_timeout = verification_timeout + 180
        fixture.kubectl('wait', '--for=condition=Ready', f'pod/{POD}', '-n', NAMESPACE,
                        f'--timeout={restart_timeout}s', timeout=restart_timeout + 30)
        actual = fixture.kubectl('exec', '-n', NAMESPACE, POD, '--', 'sha256sum', '/scratch/payload', timeout=verification_timeout).split()[0]
        if actual != expected:raise ValueError('Cross-disk file bytes changed after VM restart')
        if quantity(node_after['status']['capacity']['ephemeral-storage']) != capacity:
            raise ValueError('Kubernetes capacity changed after VM restart')
        evidence = {'result': 'passed', 'limaVersion': version, 'k3sVersion': runtime['k3sVersion'],
                    'image': IMAGE, 'memberCount': len(pool['disks']), 'largestMemberBytes': largest,
                    'filesystemCapacityBytes': pool['capacityBytes'], 'capacityBytes': capacity,
                    'allocatableBytes': allocatable, 'verifiedFileBytes': write_bytes, 'sha256': actual,
                    'initialClockSkewSeconds': round(clock_skew, 2), 'vmRestartPreservedBytes': True}
        print('NATIVE_KUBERNETES_STORAGE=' + json.dumps(evidence, sort_keys=True), flush=True)
    finally:
        if started:
            fixture.verify_owner()
            fixture.shell('systemctl', 'disable', '--now', SERVICE, timeout=120)


if __name__ == '__main__':main()
