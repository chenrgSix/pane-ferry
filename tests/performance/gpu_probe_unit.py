#!/usr/bin/env python3
"""Checks for interpretation errors that could turn diagnostics into false gains."""
import unittest

from gpu_publisher_summary import details, fence_summary, va_summary


def fence_row(signal, exported=1_000_000, returned=2_000_000):
    return dict(readback_detail=[
        dict(stage='linear.fence_export', timestamp_ns=exported),
        dict(stage='linear.fence_ready', timestamp_ns=returned),
        dict(stage='linear.fence_info', timestamp_ns=returned + 1, status=1,
             signals=[dict(status=1, signal_ns=signal)]),
    ])


class Interpretation(unittest.TestCase):
    def test_old_signaled_fence_is_not_a_thread_wake_delay(self):
        result = fence_summary([fence_row(1), fence_row(1_500_000)])
        self.assertEqual(result['already_signaled_at_export'], 1)
        self.assertEqual(result['export_to_kernel_signal_ms']['samples'], 2)
        wake = result['kernel_signal_to_poll_return_ms']
        self.assertEqual(wake['samples'], 1)
        self.assertEqual(wake['p50'], .5)

    def test_signal_cannot_follow_poll_completion(self):
        with self.assertRaises(ValueError):
            fence_summary([fence_row(3_000_000)])

    def test_thread_cpu_clocks_are_never_subtracted_across_threads(self):
        rows = [dict(codec_detail=[
            dict(stage='a', timestamp_ns=1, thread_id=5, thread_cpu_ns=100),
            dict(stage='b', timestamp_ns=2, thread_id=6, thread_cpu_ns=20),
        ])]
        result = details(rows, 'codec_detail', {'example': ('a', 'b')})
        self.assertIsNone(result['example']['same_thread_cpu_ms'])

    def test_reused_image_buffer_id_is_distinguished_from_coded_output(self):
        names = [('vaMapBuffer2', 1024), ('vaUnmapBuffer', 0),
                 ('vaBeginPicture', 0), ('vaSyncSurface', 0),
                 ('vaMapBuffer2', 0), ('vaUnmapBuffer', 0)]
        events = [dict(event=name, aux=aux, object=14, tid=7,
                       begin_ns=n * 10 + 1, end_ns=n * 10 + 2, cpu_ns=1, status=0)
                  for n, (name, aux) in enumerate(names)]
        row = dict(encode_call_start_ns=0, encode_end_ns=100,
                   codec_detail=[dict(stage='codec.sink', thread_id=7)])
        result = va_summary([row], events)['api']
        for name in ('vaMapBuffer2.image', 'vaUnmapBuffer.image',
                     'vaMapBuffer2.coded', 'vaUnmapBuffer.coded', 'vaSyncSurface.output'):
            self.assertEqual(result[name]['wall_ms']['samples'], 1)


if __name__ == '__main__':
    unittest.main()
