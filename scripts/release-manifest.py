#!/usr/bin/env python3
"""Record the exact revision, toolchain and hashes of staged release packages."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tomllib

root = Path(__file__).resolve().parents[1]
os.chdir(root)
name = sys.argv[1]
assert name in {"windows", "macos", "linux"}
version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
if os.environ.get("GITHUB_REF_TYPE") == "tag":
    assert os.environ["GITHUB_REF_NAME"] == "v" + version
packages = []
for pattern in sys.argv[2:]:
    matches = sorted(root.glob(pattern))
    assert matches, f"missing release asset: {pattern}"
    for path in matches:
        assert path.is_file() and path.stat().st_size > 1024, path
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        packages.append({"name": path.name, "size": path.stat().st_size, "sha256": digest})

def command(*args):
    return subprocess.check_output(args, text=True, encoding="utf-8").strip()

record = {
    "version": version,
    "source_commit": command("git", "rev-parse", "HEAD"),
    "platform": name,
    "system": platform.platform(),
    "architecture": platform.machine(),
    "rustc": command("rustc", "--version"),
    "built_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "workflow_run": os.environ.get("GITHUB_RUN_ID"),
    "packages": packages,
}
Path("dist").mkdir(exist_ok=True)
Path(f"dist/BUILD-INFO-{name}.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
print(json.dumps(record, indent=2))
