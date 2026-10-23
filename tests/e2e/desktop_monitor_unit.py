#!/usr/bin/env python3
"""DisplayConfig fixtures; no GNOME session or user applications are touched."""
import importlib.util
from pathlib import Path
import unittest

source = Path(__file__).resolve().parents[2] / 'apps/linux-agent/src/desktop_helper.py'
spec = importlib.util.spec_from_file_location('desktop_helper', source)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def physical(connector, width, height, *, current=True):
    return ((connector, 'Vendor', connector, 'Serial'),
            [('mode', width, height, 60.0, 1.0, [1.0, 2.0], {'is-current': current})],
            {'display-name': connector})


class MonitorListTests(unittest.TestCase):
    def test_primary_and_secondary_keep_independent_physical_and_logical_geometry(self):
        state = (5,
                 [physical('eDP-1', 2880, 1800), physical('HDMI-1', 1920, 1080)],
                 [(0, 0, 2.0, 0, True, [('eDP-1', 'Vendor', 'eDP', 'Serial')], {}),
                  (1440, 0, 1.0, 0, False, [('HDMI-1', 'Vendor', 'HDMI', 'Serial')], {})],
                 {'layout-mode': 1})
        self.assertEqual(module.monitor_list(state), [
            {'id': 'eDP-1', 'name': 'eDP-1', 'width': 2880, 'height': 1800,
             'logical_width': 1440, 'logical_height': 900, 'x': 0, 'y': 0, 'primary': True},
            {'id': 'HDMI-1', 'name': 'HDMI-1', 'width': 1920, 'height': 1080,
             'logical_width': 1920, 'logical_height': 1080, 'x': 1440, 'y': 0, 'primary': False},
        ])

    def test_rotation_and_absent_current_mode(self):
        state = (6, [physical('DP-1', 1080, 1920), physical('HDMI-2', 1920, 1080, current=False)],
                 [(-1000, 0, 1.0, 1, True, [('DP-1', 'V', 'P', 'S')], {}),
                  (0, 0, 1.0, 0, False, [('HDMI-2', 'V', 'P', 'S')], {})],
                 {'layout-mode': 1})
        self.assertEqual(module.monitor_list(state), [
            {'id': 'DP-1', 'name': 'DP-1', 'width': 1920, 'height': 1080,
             'logical_width': 1920, 'logical_height': 1080, 'x': -1000, 'y': 0, 'primary': True},
        ])

    def test_invalid_or_oversized_monitors_are_not_advertised(self):
        state = (1, [physical('../bad', 1920, 1080), physical('DP-2', 4096, 3000)],
                 [(0, 0, 1.0, 0, True, [('../bad', '', '', '')], {}),
                  (0, 0, 1.0, 0, False, [('DP-2', '', '', '')], {})],
                 {'layout-mode': 1})
        self.assertEqual(module.monitor_list(state), [])


if __name__ == '__main__':
    unittest.main()
