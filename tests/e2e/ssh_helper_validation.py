#!/usr/bin/env python3
"""Invalid connection inputs must be rejected before SSH is invoked."""
import pathlib
import subprocess

helper = pathlib.Path(__file__).resolve().parents[2] / "scripts/connect-ssh.sh"
cases = [[], ["host"], ["host;id", "alice"], ["a..b", "alice"],
         ["-oProxyCommand=evil", "alice"], ["[::1]", "alice"],
         ["host", "alice;id"], ["host", "-alice"], ["host", "alice", "0"],
         ["host", "alice", "65536"], ["host", "alice", "22;id"]]
for arguments in cases:
    result = subprocess.run(["bash", str(helper), *arguments], capture_output=True, timeout=2)
    assert result.returncode == 2, f"Invalid input accepted: {arguments!r}"
print(f"PASS: {len(cases)} invalid SSH helper inputs rejected")
