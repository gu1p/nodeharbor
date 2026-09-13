#!/usr/bin/env python3
"""Fixed owner commands over the managed Android guest's private virtio port."""
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import threading
import uuid
from watchdog import renew_lease as renew_owner_lease

LIMIT = 1024 * 1024
CONTROL_REVISION = '4'
ROOT = Path('/etc/nodeharbor')
PORT = '/dev/virtio-ports/io.github.gu1p.nodeharbor.control'
EVENTS = set()


def control_revision():
    # A seed can replace this file before systemd reloads the old unit. Keep
    # answering leases, but advertise readiness only after activation of the
    # matching unit and its normal-poweroff dependencies.
    return CONTROL_REVISION if os.environ.get('NODEHARBOR_CONTROL_UNIT_REVISION') == CONTROL_REVISION else ''


def lifecycle_event(event):
    if event not in {'status-received', 'lease-received', 'configure-received',
                     'poweroff-received', 'shutdown-initiated', 'shutdown-request-failed'}:
        raise ValueError('Unsupported lifecycle event')
    if event not in EVENTS:
        EVENTS.add(event)
        print('NODEHARBOR_CONTROL ' + event, flush=True)


def notify_ready():
    address = os.environ['NOTIFY_SOCKET']
    if address.startswith('@'): address = '\0' + address[1:]
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as notification:
        notification.connect(address)
        notification.sendall(b'READY=1')


def exact(stream, size):
    result = bytearray()
    while len(result) < size:
        block = stream.read(size - len(result))
        if not block: raise EOFError('The owner channel closed')
        result.extend(block)
    return bytes(result)


def read_frame(stream):
    size, = struct.unpack('>I', exact(stream, 4))
    if not 2 <= size <= LIMIT: raise ValueError('Invalid owner frame size')
    value = json.loads(exact(stream, size))
    if not isinstance(value, dict): raise ValueError('Expected an owner command object')
    return value


def write_frame(stream, value):
    data = json.dumps(value, separators=(',', ':'), allow_nan=False).encode()
    if len(data) > LIMIT: raise ValueError('Owner response exceeds its supported size')
    packet = struct.pack('>I', len(data)) + data
    offset = 0
    while offset < len(packet):
        count = stream.write(packet[offset:])
        if not count: raise EOFError('The owner channel closed')
        offset += count
    stream.flush()


def validate_request(value, owner):
    if value.get('deviceId') != owner or str(uuid.UUID(owner)) != owner:
        raise ValueError('The guest belongs to a different owner')
    if type(value.get('id')) is not int or not 0 <= value['id'] < 2**53:
        raise ValueError('Invalid owner request number')
    command = value.get('command')
    if command not in {'status', 'lease', 'configure', 'poweroff'}:
        raise ValueError('Unsupported owner command')
    keys = {'id', 'deviceId', 'command'} | ({'bootstrap'} if command == 'configure' else set())
    if set(value) != keys: raise ValueError('Unsupported owner command fields')
    if command == 'configure':
        config = value['bootstrap']
        if not isinstance(config, dict) or config.get('deviceId') != owner:
            raise ValueError('Bootstrap belongs to a different owner')
    return value


def pod_inventory(text):
    if len(text) > LIMIT: raise ValueError('Workload inventory exceeds its supported size')
    items = json.loads(text).get('items')
    if not isinstance(items, list) or len(items) > 4096: raise ValueError('Invalid workload inventory')
    pods = []
    for item in items:
        metadata = item.get('metadata', {})
        uid, namespace = metadata.get('uid'), metadata.get('namespace')
        if (item.get('state') not in {'SANDBOX_READY', 'SANDBOX_NOTREADY'} or
                not isinstance(uid, str) or not 1 <= len(uid) <= 128 or
                not isinstance(namespace, str) or not 1 <= len(namespace) <= 253):
            raise ValueError('Unknown workload state')
        pods.append({'uid': uid, 'namespace': namespace})
    return pods


class Guest:
    def __init__(self, owner):
        self.owner = owner
        self.lock = threading.Lock()
        self.preparing = False
        self.error = ''
        self.progress = ''
        self.configured = (ROOT / 'android-configured').is_file()

    def configure(self, bootstrap):
        with self.lock:
            if self.preparing: raise ValueError('Worker preparation is already running')
            self.preparing, self.configured, self.error = True, False, ''
        threading.Thread(target=self._configure, args=(bootstrap,), daemon=True).start()

    def _configure(self, bootstrap):
        try:
            # The controller grant is read from stdin. No credentials appear in
            # arguments, environment, console output, or the owner status response.
            with subprocess.Popen(['/usr/bin/python3', '/usr/local/lib/nodeharbor/configure_worker.py'],
                                  stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) as child:
                try:
                    child.communicate(json.dumps(bootstrap).encode(), timeout=1200)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
                    raise
                if child.returncode: raise RuntimeError('Worker preparation failed')
            marker = ROOT / 'android-configured'
            temporary = marker.with_suffix('.new')
            temporary.write_text(self.owner)
            temporary.chmod(0o600)
            temporary.replace(marker)
            with self.lock: self.configured = True
        except Exception:
            with self.lock: self.error = 'Worker preparation failed; check fleet connectivity and retry'
        finally:
            with self.lock: self.preparing = False

    def status(self):
        with self.lock: configured = self.configured
        active = False
        if configured and control_revision():
            try:
                active = subprocess.run(['systemctl', 'is-active', '--quiet', 'k3s-agent'], timeout=10).returncode == 0
            except (OSError, subprocess.TimeoutExpired):
                # Early boot or an unavailable service manager must still allow
                # owner leases. Unknown service state is never healthy or empty.
                pass
        pods = None
        if active:
            try:
                # Bound output while reading so a broken runtime cannot exhaust
                # guest memory or publish a false empty workload inventory.
                with subprocess.Popen(['/usr/local/bin/k3s', 'crictl', 'pods', '-o', 'json'],
                                      stdout=subprocess.PIPE, stderr=subprocess.DEVNULL) as child:
                    timer = threading.Timer(10, child.kill)
                    timer.start()
                    try:
                        output = child.stdout.read(LIMIT + 1)
                        if len(output) > LIMIT: child.kill()
                        if child.wait() == 0: pods = pod_inventory(output.decode())
                    finally: timer.cancel()
            except Exception: pass
        with self.lock:
            return {'configured': self.configured, 'preparing': self.preparing, 'error': self.error,
                    'running': active, 'pods': pods, 'architecture': os.uname().machine,
                    'controlRevision': control_revision()}


def main():
    if sys.platform != 'linux' or os.geteuid() != 0:
        raise SystemExit('The owner channel only runs inside its managed Linux guest')
    owner = (ROOT / 'device-id').read_text().strip()
    if str(uuid.UUID(owner)) != owner: raise SystemExit('Invalid guest ownership')
    guest = Guest(owner)
    with open(PORT, 'r+b', buffering=0) as channel:
        notify_ready()
        while True:
            request = validate_request(read_frame(channel), owner)
            command = request['command']
            lifecycle_event(command + '-received')
            result = {'id': request['id'], 'deviceId': owner}
            try:
                if command == 'lease':
                    renew_owner_lease()
                elif command == 'configure': guest.configure(request['bootstrap'])
                elif command == 'status': result.update(guest.status())
                result['ok'] = True
            except Exception:
                result.update(ok=False, error='The guest could not complete the owner command')
            write_frame(channel, result)
            if command == 'poweroff':
                lifecycle_event('shutdown-initiated')
                try: subprocess.run(['systemctl', 'poweroff'], check=True, timeout=10)
                except Exception:
                    lifecycle_event('shutdown-request-failed')
                    raise
                return


if __name__ == '__main__': main()
