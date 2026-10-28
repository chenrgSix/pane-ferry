#!/usr/bin/env python3
"""Validate same-frame publisher traces and summarize the measured receive window."""
import argparse
import json
from pathlib import Path

from local_scroll import distribution


STAGES = (
    'capture_start_ns', 'capture_end_ns', 'publish_ns', 'encode_enter_ns',
    'encode_call_start_ns', 'encode_end_ns', 'serialize_start_ns',
    'write_begin_ns', 'write_end_ns',
)
INTERVALS = {
    'capture': ('capture_start_ns', 'capture_end_ns'),
    'capture_complete_to_publish_queue': ('capture_end_ns', 'publish_ns'),
    'encoder_queue': ('publish_ns', 'encode_enter_ns'),
    'encoder_preparation': ('encode_enter_ns', 'encode_call_start_ns'),
    'conversion_and_encoding': ('encode_call_start_ns', 'encode_end_ns'),
    'encoded_to_serialization': ('encode_end_ns', 'serialize_start_ns'),
    'serialization': ('serialize_start_ns', 'write_begin_ns'),
    'socket_write': ('write_begin_ns', 'write_end_ns'),
    'capture_complete_to_write': ('capture_end_ns', 'write_end_ns'),
    'capture_start_to_write': ('capture_start_ns', 'write_end_ns'),
}


def matched_frames(received, trace):
    by_sequence = {}
    for row in trace:
        if row['sequence'] in by_sequence:
            raise ValueError('duplicate trace sequence; use one Agent run per trace')
        by_sequence[row['sequence']] = row
    rows = []
    ids, sequences = set(), set()
    for frame in received['frames']:
        sequence = frame['sequence']
        if sequence in sequences:
            raise ValueError('duplicate received sequence')
        sequences.add(sequence)
        row = by_sequence[sequence]  # Missing trace rows must fail, not disappear.
        if row['capture_id'] in ids:
            raise ValueError('a captured snapshot was published more than once')
        ids.add(row['capture_id'])
        if row['json_bytes'] != frame['json_bytes']:
            raise ValueError('trace and received message sizes differ')
        stamps = [row[key] for key in STAGES]
        if not all(isinstance(stamp, int) and stamp > 0 for stamp in stamps):
            raise ValueError('invalid stage timestamp')
        if stamps != sorted(stamps):
            raise ValueError('non-monotonic stages')
        # A receiver may see bytes before the writer resumes to stamp write_end.
        if frame['ns'] < row['write_begin_ns']:
            raise ValueError('receive time precedes the write call')
        rows.append(row)
    if not rows:
        raise ValueError('no measured frames')
    return rows


def summarize(rows):
    return dict(
        published_frames=len(rows),
        capture_backends=sorted({row['capture_backend'] for row in rows}),
        milliseconds={name: distribution([(row[end] - row[start]) / 1e6 for row in rows])
                      for name, (start, end) in INTERVALS.items()},
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('received', type=Path)
    parser.add_argument('trace', type=Path)
    args = parser.parse_args()
    received = json.loads(args.received.read_text())
    trace = [json.loads(line) for line in args.trace.read_text().splitlines()]
    print(json.dumps(summarize(matched_frames(received, trace)), indent=2))


if __name__ == '__main__':
    main()
