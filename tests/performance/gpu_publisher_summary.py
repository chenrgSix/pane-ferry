#!/usr/bin/env python3
"""Summarize same-frame readback, codec-pad and optional VA API diagnostics."""
import argparse
import json
from pathlib import Path

from local_scroll import distribution
from publisher_stage_summary import matched_frames, summarize


READBACK = {
    'name_source': ('capture.begin', 'source.named'),
    'check_geometry': ('source.named', 'source.geometry_checked'),
    'submit_render': ('source.geometry_checked', 'render.submitted'),
    'flush_and_export': ('linear.submit_begin', 'linear.export_received'),
    'wait_for_readable': ('linear.export_received', 'linear.fence_ready'),
    'before_copy': ('linear.fence_ready', 'linear.copy_begin'),
    'copy_snapshot': ('linear.copy_begin', 'linear.copy_end'),
    'end_cpu_access': ('linear.copy_end', 'linear.coherency_ended'),
}
CODEC = {
    'wrap_input': ('encode.begin', 'input.wrapped'),
    'convert_color': ('conversion.sink', 'conversion.src'),
    'interleave_nv12': ('interleave.sink', 'interleave.src'),
    'va_postprocess': ('upload.sink', 'upload.src'),
    'encoder_element': ('codec.sink', 'codec.src'),
    'deliver_sample': ('appsink.sink', 'output.pulled'),
}


def stats(values):
    return dict(samples=len(values), **distribution(values))


def details(rows, group, intervals):
    result = {}
    for name, (start, end) in intervals.items():
        wall, cpu = [], []
        for row in rows:
            points = row[group]
            by_name = {p['stage']: p for p in points}
            if len(by_name) != len(points):
                raise ValueError('duplicate detail point; frame association is ambiguous')
            if start not in by_name or end not in by_name:
                continue
            a, b = by_name[start], by_name[end]
            if b['timestamp_ns'] < a['timestamp_ns']:
                raise ValueError('negative detail interval')
            wall.append((b['timestamp_ns'] - a['timestamp_ns']) / 1e6)
            if a.get('thread_id') == b.get('thread_id') and 'thread_cpu_ns' in a and 'thread_cpu_ns' in b:
                if b['thread_cpu_ns'] < a['thread_cpu_ns']:
                    raise ValueError('negative thread CPU interval')
                cpu.append((b['thread_cpu_ns'] - a['thread_cpu_ns']) / 1e6)
        if wall:
            result[name] = dict(wall_ms=stats(wall), same_thread_cpu_ms=stats(cpu) if cpu else None)
    return result


def fence_summary(rows):
    wait, wake = [], []
    missing, already_ready = 0, 0
    for row in rows:
        points = {p['stage']: p for p in row['readback_detail']}
        exported, info = points.get('linear.fence_export'), points.get('linear.fence_info')
        if not exported or not info or info['status'] != 1 or not info['signals']:
            missing += 1
            continue
        signals = info['signals']
        if any(s['status'] != 1 or s['signal_ns'] <= 0 for s in signals):
            missing += 1
            continue
        signal = max(s['signal_ns'] for s in signals)
        ready = points['linear.fence_ready']['timestamp_ns']
        if signal > ready:
            raise ValueError('kernel signal timestamp follows poll return')
        wait.append(max(0, signal - exported['timestamp_ns']) / 1e6)
        if signal <= exported['timestamp_ns']:
            # A retired/stub fence may have a very old timestamp. No wait took
            # place, so its age is not a thread wake-up delay.
            already_ready += 1
        else:
            wake.append((ready - signal) / 1e6)
    return dict(unavailable_frames=missing, already_signaled_at_export=already_ready,
                export_to_kernel_signal_ms=stats(wait) if wait else None,
                kernel_signal_to_poll_return_ms=stats(wake) if wake else None)


def va_summary(rows, events):
    groups, total_calls = {}, 0
    for row in rows:
        calls = [v for v in events if row['encode_call_start_ns'] <= v['begin_ns']
                 and v['end_ns'] <= row['encode_end_ns']]
        # One encode is in flight per window; use pad thread to reject another
        # process/codec's calls if a diagnostic accidentally includes them.
        points = {p['stage']: p for p in row['codec_detail']}
        tid = points['codec.sink']['thread_id']
        calls = sorted((v for v in calls if v['tid'] == tid), key=lambda v: v['begin_ns'])
        if not calls:
            raise ValueError('no matching VA calls for a measured hardware frame')
        begun = False
        maps = {}
        for call in calls:
            if call['status'] != 0:
                raise ValueError('VA call failed')
            name = call['event']
            if name == 'vaBeginPicture':
                begun = True
            if name in ('vaSyncSurface', 'vaSyncSurface2'):
                name += '.output' if begun else '.input'
            if name in ('vaMapBuffer', 'vaMapBuffer2'):
                maps[call['object']] = 'image' if call['aux'] else 'coded'
                name += '.' + maps[call['object']]
            if name == 'vaUnmapBuffer':
                name += '.' + maps[call['object']]
            groups.setdefault(name, []).append(call)
            total_calls += 1
    return dict(matched_calls=total_calls, api={name: dict(
        wall_ms=stats([(c['end_ns'] - c['begin_ns']) / 1e6 for c in calls]),
        same_thread_cpu_ms=stats([c['cpu_ns'] / 1e6 for c in calls]))
        for name, calls in groups.items()})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('received', type=Path)
    parser.add_argument('trace', type=Path)
    parser.add_argument('--va-trace', type=Path)
    args = parser.parse_args()
    received = json.loads(args.received.read_text())
    rows = matched_frames(received, [json.loads(line) for line in args.trace.read_text().splitlines()])
    result = summarize(rows)
    result['readback'] = details(rows, 'readback_detail', READBACK)
    result['codec'] = details(rows, 'codec_detail', CODEC)
    result['fence'] = fence_summary(rows)
    if args.va_trace:
        result['va'] = va_summary(rows, [json.loads(line) for line in args.va_trace.read_text().splitlines()])
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
