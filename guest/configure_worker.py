#!/usr/bin/env python3
"""Configure an owned worker from a short-lived, authenticated bootstrap grant."""
import hashlib
import io
import ipaddress
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse
import urllib.request
import uuid

ROOT=Path('/etc/nodeharbor')
RESOLV_CONF=Path('/run/systemd/resolve/resolv.conf')

def validate_resolver(path):
    try:lines=path.read_text().splitlines()
    except OSError as error:raise ValueError('The guest upstream resolver is unavailable') from error
    count=0
    for line in lines:
        fields=line.split('#',1)[0].split()
        if not fields or fields[0]!='nameserver':continue
        try:
            if len(fields)!=2:raise ValueError('Malformed nameserver')
            address=ipaddress.ip_address(fields[1])
        except ValueError as error:raise ValueError('The guest upstream resolver has an invalid nameserver') from error
        if address.is_loopback or address.is_link_local or address.is_multicast or address.is_unspecified:
            raise ValueError('The guest upstream resolver is not usable from Kubernetes pods')
        count+=1
    if not count:raise ValueError('The guest upstream resolver has no nameservers')

def validate_config(config):
    device=uuid.UUID(config['deviceId'])
    if config['nodeName']!='nodeharbor-'+device.hex:
        raise ValueError('Worker identity does not match its enrollment')
    for key in ['netbirdManagementUrl','serverUrl']:
        url=urllib.parse.urlparse(config[key])
        if url.scheme!='https' or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError('Worker endpoints must use HTTPS without embedded credentials')
    if not re.fullmatch(r'K10[0-9a-f]{64}::[a-z0-9]{6}\.[a-z0-9]{16}',config['k3sToken']):
        raise ValueError('A secure expiring K3s bootstrap token is required')
    runtime=config['runtime']
    if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+\+k3s[0-9]+',runtime['k3sVersion']):
        raise ValueError('Invalid K3s runtime version')
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+',runtime['netbirdVersion']):
        raise ValueError('Invalid NetBird runtime version')

def verify_download(data,expected):
    if not re.fullmatch(r'[0-9a-f]{64}',expected) or hashlib.sha256(data).hexdigest()!=expected:
        raise ValueError('Downloaded runtime failed checksum verification')

def run(*command,env=None):
    result=subprocess.run(command,env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    if result.returncode:
        raise RuntimeError(f'{command[0]} failed; inspect the worker service logs')
    return result.stdout

def write(path,content,mode=0o600):
    write_bytes(path,content.encode(),mode)

def write_bytes(path,content,mode=0o600):
    path=Path(path);path.parent.mkdir(parents=True,exist_ok=True)
    name=None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent,delete=False) as temporary:
            name=temporary.name
            temporary.write(content);temporary.flush();os.fsync(temporary.fileno())
        os.chmod(name,mode);os.replace(name,path)
    finally:
        if name is not None:Path(name).unlink(missing_ok=True)

def download(url,expected):
    with urllib.request.urlopen(url,timeout=120) as response:
        data=response.read(512*1024*1024+1)
    if len(data)>512*1024*1024: raise ValueError('Worker runtime download exceeds the supported size')
    verify_download(data,expected)
    return data

def cached_download(url,expected,cache_dir):
    if not re.fullmatch(r'[0-9a-f]{64}',expected):raise ValueError('Invalid runtime checksum')
    cache_dir.mkdir(parents=True,exist_ok=True,mode=0o700)
    cached=cache_dir/expected
    if cached.is_file() and cached.stat().st_size<=512*1024*1024:
        data=cached.read_bytes()
        try:verify_download(data,expected);return data
        except ValueError:pass
    data=download(url,expected)
    verify_download(data,expected)
    write_bytes(cached,data)
    return data

def install_runtime(config,bin_dir=Path('/usr/local/bin'),cache_dir=Path('/var/cache/nodeharbor/runtime')):
    runtime=config['runtime'];arch={'aarch64':'arm64','x86_64':'amd64'}.get(platform.machine())
    if arch is None: raise ValueError('Unsupported Linux worker CPU architecture')
    assets={asset['name']:asset['sha256'] for asset in runtime['assets']}
    print('Installing verified worker runtime',flush=True)
    nb_name=f"netbird_{runtime['netbirdVersion']}_linux_{arch}.tar.gz"
    data=cached_download(f"https://github.com/netbirdio/netbird/releases/download/v{runtime['netbirdVersion']}/{nb_name}",assets[nb_name],cache_dir)
    with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as archive:
        members=[member for member in archive.getmembers() if member.name in ('netbird','./netbird') and member.isfile()]
        if len(members)!=1:raise ValueError('NetBird archive has no unique executable')
        if members[0].size>512*1024*1024:raise ValueError('NetBird executable exceeds the supported size')
        netbird_binary=archive.extractfile(members[0]).read()
    k3s_name='k3s' if arch=='amd64' else 'k3s-arm64'
    k3s_binary=cached_download(f"https://github.com/k3s-io/k3s/releases/download/{urllib.parse.quote(runtime['k3sVersion'],safe='')}/{k3s_name}",assets[k3s_name],cache_dir)
    # Verify both downloads before changing either installed runtime. Rename on the
    # same filesystem preserves executables already mapped by running processes.
    for name,binary in [('netbird',netbird_binary),('k3s',k3s_binary)]:
        path=bin_dir/name
        if path.is_file() and path.stat().st_size==len(binary) and path.read_bytes()==binary:
            path.chmod(0o755)
        else:write_bytes(path,binary,0o755)

def main():
    if sys.platform!='linux' or os.geteuid()!=0:
        raise SystemExit('Worker configuration only runs as root inside a managed Linux guest')
    config=json.load(sys.stdin);validate_config(config)
    if (ROOT/'device-id').read_text().strip()!=config['deviceId']:
        raise SystemExit('Refusing to configure a VM owned by another device')
    # Use Ubuntu's DHCP-provided upstream DNS. NetBird must not replace it, and
    # invalid DNS must not silently fall back to an unrelated public resolver.
    validate_resolver(RESOLV_CONF)
    install_runtime(config)
    if not Path('/etc/systemd/system/netbird.service').exists():run('/usr/local/bin/netbird','service','install')
    run('systemctl','enable','--now','netbird')
    environment=dict(os.environ)
    if config.get('netbirdSetupKey'):environment['NB_SETUP_KEY']=config['netbirdSetupKey']
    run('/usr/local/bin/netbird','up','--management-url',config['netbirdManagementUrl'],'--hostname',config['nodeName'],'--mtu','1280',
        '--disable-dns','--disable-server-routes','--disable-client-routes=false','--allow-server-ssh=false',env=environment)
    peer_ip=None
    for _ in range(60):
        status=json.loads(run('/usr/local/bin/netbird','status','--json'))
        value=status.get('netbirdIp','').split('/')[0]
        try:peer_ip=str(ipaddress.IPv4Address(value));break
        except ipaddress.AddressValueError:time.sleep(1)
    if not peer_ip:raise RuntimeError('The private network did not assign a worker address')
    for module in ['overlay','br_netfilter','vxlan']:run('modprobe',module)
    write('/etc/modules-load.d/nodeharbor.conf','overlay\nbr_netfilter\nvxlan\n',0o644)
    write('/etc/sysctl.d/90-nodeharbor.conf','net.ipv4.ip_forward=1\nnet.bridge.bridge-nf-call-iptables=1\n',0o644)
    run('sysctl','--system')
    write('/etc/rancher/k3s/agent-token',config['k3sToken']+'\n')
    k3s={'server':config['serverUrl'],'token-file':'/etc/rancher/k3s/agent-token','node-name':config['nodeName'],'node-ip':peer_ip,'flannel-iface':'wt0',
         'node-taint':['nodeharbor.sikalio.dev/contributed=true:NoSchedule','nodeharbor.sikalio.dev/quarantine=true:NoSchedule'],
         'node-label':['nodeharbor.sikalio.dev/device='+config['deviceId']],
         'resolv-conf':str(RESOLV_CONF),
         'kubelet-arg':['system-reserved=cpu=100m,memory=256Mi','kube-reserved=cpu=150m,memory=256Mi','eviction-hard=memory.available<256Mi,nodefs.available<10%,imagefs.available<15%','container-log-max-size=10Mi','container-log-max-files=2','max-pods=30']}
    write('/etc/rancher/k3s/config.yaml',json.dumps(k3s,indent=2)+'\n')
    unit='''[Unit]
Description=NodeHarbor Kubernetes worker
After=network-online.target netbird.service
Wants=network-online.target
Requires=netbird.service
[Service]
Type=notify
ExecStart=/usr/local/bin/k3s agent --config /etc/rancher/k3s/config.yaml
Restart=always
RestartSec=10
Delegate=yes
LimitNOFILE=1048576
TasksMax=infinity
[Install]
WantedBy=multi-user.target
'''
    write('/etc/systemd/system/k3s-agent.service',unit,0o644)
    run('systemctl','daemon-reload')
    run('systemctl','enable','--now','nodeharbor-watchdog.timer')
    run('systemctl','enable','--now','k3s-agent')
    print('Worker connected; waiting for controller qualification',flush=True)

if __name__=='__main__': main()
