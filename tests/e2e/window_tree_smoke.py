#!/usr/bin/env python3
"""Exercise real AppKit window hierarchy, placement and input routing."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-window-tree-') as directory:
    scratch = Path(directory)
    types = scratch/'WindowHostTypes.swift'
    text = (source/'main.swift').read_text()
    marker = '\n// WINDOWHOST_ENTRYPOINT\n'
    assert text.count(marker) == 1
    types.write_text(text.split(marker)[0])
    binary = scratch/'smoke'
    files = [str(path) for path in source.glob('*.swift') if path.name != 'main.swift']
    subprocess.run(['swiftc', '-module-cache-path', str(scratch/'modules'), *files, str(types),
                    str(root/'tests/swift/WindowTreeSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=25)
    if result.returncode:
        print(result.stdout, result.stderr[-4000:], flush=True)

    assert result.returncode == 0, result.returncode
    print(result.stdout, end='')
