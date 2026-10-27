#!/usr/bin/env python3
"""Portable tests for authorization storage and shared input ownership."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

source = Path(__file__).resolve().parents[2] / 'apps/linux-agent/src/desktop_input.py'
spec = importlib.util.spec_from_file_location('desktop_input', source)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class AuthorizationTests(unittest.TestCase):
    def test_sync_pong_disconnect_timeout_and_cleanup(self):
        class Native:
            def __init__(self): self.events = []
            def ei_new_ping(self, context): return 7
            def ei_ping(self, ping): self.events.append(('ping',ping))
            def ei_ping_unref(self, ping): self.events.append(('unref',ping))
        for outcome in ('ready','timeout','disconnected','paused'):
            with self.subTest(outcome=outcome):
                eis = module.Eis.__new__(module.Eis)
                eis.lib, eis.context, eis.can_sync = Native(), 1, True
                eis.disconnected = False
                eis.ready = lambda: outcome != 'paused'
                def dispatch():
                    eis.pong = outcome != 'timeout'
                    eis.disconnected = outcome == 'disconnected'
                eis.dispatch = dispatch
                with patch.object(module.time,'monotonic',side_effect=[0,2]):
                    if outcome == 'ready': eis.sync()
                    else:
                        with self.assertRaisesRegex(RuntimeError,'desktop_input_unavailable'):
                            eis.sync()
                self.assertIsNone(eis.waiting_ping)
                self.assertEqual(eis.lib.events,[('ping',7),('unref',7)])

    def test_token_rotation_permissions_and_corrupt_state(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            store = module.TokenStore(root)
            self.assertIsNone(store.load())
            store.replace('first-fixture-token')
            self.assertEqual(store.load(), 'first-fixture-token')
            self.assertEqual(store.path.stat().st_mode & 0o777, 0o600)
            store.replace('rotated-fixture-token')
            self.assertEqual(store.load(), 'rotated-fixture-token')
            self.assertNotIn('first-fixture-token', store.path.read_text())
            for value in ([], None, 7, {'version': 99}, {'version': 1, 'app_id': 'other', 'restore_token': 'bad'}):
                store.path.write_text(json.dumps(value))
                self.assertIsNone(store.load())
            store.path.write_bytes(b'not json')
            self.assertIsNone(store.load())
            store.replace(None)
            self.assertFalse(store.path.exists())

    def test_token_symlink_public_file_and_public_directory_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            store = module.TokenStore(root)
            outside = root / 'unrelated'
            outside.write_text('do not touch')
            store.path.symlink_to(outside)
            with self.assertRaises(OSError):
                store.load()
            store.path.unlink()
            store.replace('fixture-token')
            store.path.chmod(0o644)
            with self.assertRaises(ValueError):
                store.load()
            root.chmod(0o755)
            with self.assertRaises(ValueError):
                module.TokenStore(root)
            self.assertEqual(outside.read_text(), 'do not touch')

    def test_release_is_scoped_to_controller_without_duplicate_keyup(self):
        class Native:
            def __init__(self): self.events = []
            def ei_device_keyboard_key(self, *args): self.events.append(('key', *args))
            def ei_device_button_button(self, *args): self.events.append(('button', *args))
            def ei_device_frame(self, *args): pass
            def ei_now(self, context): return 1
        eis = module.Eis.__new__(module.Eis)
        eis.lib, eis.held, eis.context = Native(), {}, 1
        eis.active, eis.releases = {10, 11}, set()
        eis.press(1, 10, 30, True, True)
        eis.press(1, 10, 30, True, True)
        eis.press(2, 10, 30, True, True)
        eis.press(3, 11, 272, True, False)
        # A targeted key-up may bypass window queries, but it cannot release
        # another controller's key or synthesize an unheld key transition.
        eis.dispatch = lambda: None
        eis.send(dict(type='key', owner=99, code=38, pressed=False))
        self.assertEqual(eis.held[(10, 30, True)], {1, 2})
        self.assertEqual(len(eis.lib.events), 2)
        eis.release(1)
        self.assertEqual(eis.lib.events, [('key', 10, 30, True), ('button', 11, 272, True)])
        eis.release(2)
        eis.release(2)
        eis.release(3)
        self.assertEqual(eis.lib.events[-2:], [('key', 10, 30, False), ('button', 11, 272, False)])
        self.assertEqual(eis.held, {})
        eis.press(1, 10, 30, True, True)
        eis.active.remove(10)
        before = len(eis.lib.events)
        eis.release(1)
        self.assertEqual(len(eis.lib.events), before, 'release sent through a paused device')
        eis.active.add(10)
        eis.flush_releases(10)
        self.assertEqual(eis.lib.events[-1], ('key', 10, 30, False))
        self.assertEqual(eis.releases, set())


if __name__ == '__main__':
    unittest.main()
