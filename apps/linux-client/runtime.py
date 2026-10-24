"""Resolve helpers from the same installation as this GTK client."""
import os
from pathlib import Path
import subprocess


def helper_binary(name):
    if name not in ('paneferry-agent', 'paneferry-network'):
        raise ValueError('invalid_helper')
    # Debian puts Python and native helpers together in /usr/lib/paneferry.
    # The tar installer keeps its helpers in this user's ~/.local/bin.
    sibling = Path(__file__).resolve().parent / name
    candidate = sibling if sibling.is_file() else Path.home() / '.local/bin' / name
    if not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise ValueError('agent_unavailable' if name == 'paneferry-agent' else 'network_unavailable')
    return candidate


def start_agent():
    agent = helper_binary('paneferry-agent')
    if agent.parent == Path('/usr/lib/paneferry'):
        # Installation/upgrade never restarts users' applications. Start the
        # user unit on first GUI launch; containers can use the direct fallback.
        try:
            result = subprocess.run(['systemctl', '--user', 'start', 'remoteapp-agent.service'],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
            if result.returncode == 0:
                return
        except (OSError, subprocess.TimeoutExpired):
            pass
    subprocess.Popen([str(agent), 'serve'], stdin=subprocess.DEVNULL,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
