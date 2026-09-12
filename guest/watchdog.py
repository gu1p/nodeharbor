#!/usr/bin/env python3
"""Stop this managed guest when the owner agent stops renewing its lease."""
from pathlib import Path
import math
import os
import subprocess
import sys

LEASE=Path('/run/nodeharbor/lease')

def lease_expired(renewed,now,timeout=120):
    # /run is empty after boot. Give the owning agent one bounded lease period
    # to reconnect; a stopped or crashed agent cannot extend this grace.
    if renewed is None:
        return now >= timeout
    return not math.isfinite(renewed) or renewed>now or now-renewed>=timeout

def uptime():
    return float(Path('/proc/uptime').read_text().split()[0])

def main():
    if sys.platform!='linux' or os.geteuid()!=0 or not Path('/etc/nodeharbor/device-id').is_file():
        raise SystemExit('This command only runs inside a managed NodeHarbor Linux guest')
    now=uptime()
    if len(sys.argv)==2 and sys.argv[1]=='renew':
        LEASE.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
        temporary=LEASE.with_suffix('.new')
        temporary.write_text(str(now))
        temporary.replace(LEASE)
        return
    try: renewed=float(LEASE.read_text())
    except (OSError,ValueError):renewed=None
    if lease_expired(renewed,now):
        subprocess.run(['systemctl','poweroff'],check=True)

if __name__=='__main__': main()
