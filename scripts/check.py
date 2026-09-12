#!/usr/bin/env python3
"""The same checks locally and in CI; never rewrites source files."""
from pathlib import Path
import os, subprocess, sys
ROOT = Path(__file__).resolve().parents[1]
def run(command, cwd=ROOT):
    print("+ " + " ".join(command), flush=True)
    subprocess.run(command, cwd=cwd, check=True)
try:
    run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-v"])
    if os.name == "nt":
        run(["powershell", "-NoProfile", "-File", "tests/installer.Tests.ps1"])
    run(["npm.cmd" if os.name == "nt" else "npm", "run", "check"], ROOT / "ui")
    run(["cargo", "fmt", "--all", "--", "--check"])
    run(["cargo", "test", "--locked"])
    run(["cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings"])
except subprocess.CalledProcessError as error:
    sys.exit(error.returncode)
