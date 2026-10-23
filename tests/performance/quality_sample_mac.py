#!/usr/bin/env python3
"""Decode a quality_sample export and reconstruct its imageRect in backing pixels.

Explicit content points/backing density must come from the measured content area,
not the outer window. Never contacts a publisher or changes an application.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sample', type=Path, nargs='?')
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--content-points', type=float, nargs=2)
    parser.add_argument('--backing', type=float)
    args = parser.parse_args()
    if args.self_test:
        if args.sample or args.content_points or args.backing is not None:
            parser.error('--self-test cannot be combined with sample parameters')
        parameters = ['--self-test']
    else:
        if args.sample is None or args.content_points is None or args.backing is None:
            parser.error('sample, --content-points and --backing are required')
        parameters = [str(args.sample.resolve()), *map(str, args.content_points), str(args.backing)]
    root = Path(__file__).resolve().parents[2]
    source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
    with tempfile.TemporaryDirectory(prefix='paneferry-quality-', dir='/tmp') as temp:
        binary = Path(temp) / 'quality-sample'
        subprocess.run(['swiftc', '-O', '-module-cache-path', str(Path(temp) / 'modules'),
                        *[str(source / name) for name in ('H264Decoder.swift', 'SurfaceView.swift', 'RenderGeometry.swift')],
                        str(Path(__file__).with_name('QualitySample.swift')), '-o', str(binary)],
                       check=True, timeout=120)
        subprocess.run([str(binary), *parameters],
                       check=True, timeout=60)


if __name__ == '__main__':
    main()
