#!/usr/bin/env python3
"""Measure the generated English login screen in chatgpt_density_probe exports.

Uses source pixels only, not compositor scale metadata. Refuses an unexpected
screen; these measurements do not establish authenticated-content behavior.
Requires Pillow. Never contacts or controls an application.
"""
import argparse
import json
from pathlib import Path
from PIL import Image, ImageChops, ImageDraw, ImageStat


def measure(path, logical):
    image = Image.open(path).convert('RGB')
    width, height = image.size
    assert min(image.getpixel((width // 10, height // 10))) > 240, 'expected fresh light login UI'
    mask = image.convert('L').point(lambda p: 255 if p < 128 else 0)
    # The solid login button has more dark pixels than any text row. Select its
    # connected background, preserving the white text holes inside the button.
    region = mask.crop((width // 5, height // 5, width * 4 // 5, height * 17 // 20))
    row = max(range(region.height), key=lambda y: sum(region.crop((0, y, region.width, y + 1)).tobytes())) + height // 5
    assert mask.getpixel((width // 2, row)) == 255
    ImageDraw.floodfill(mask, (width // 2, row), 128)
    button = mask.point(lambda p: 255 if p == 128 else 0).getbbox()
    assert button and 300 <= button[2] - button[0] <= 720, 'unexpected login button'
    glyph_scale = (button[2] - button[0]) / 340
    heading_area = (width // 5, max(0, round(button[1] - 80 * glyph_scale)),
                    width * 4 // 5, round(button[1] - 20 * glyph_scale))
    heading = image.crop(heading_area).convert('L').point(lambda p: 255 if p < 128 else 0).getbbox()
    assert heading, 'missing login heading'
    heading = (heading[0] + heading_area[0], heading[1] + heading_area[1],
               heading[2] + heading_area[0], heading[3] + heading_area[1])
    sx, sy = width / logical[0], height / logical[1]
    result = dict(source_pixels=[width, height], button_pixels=list(button), heading_pixels=list(heading),
                  button_logical_size=[(button[2] - button[0]) / sx, (button[3] - button[1]) / sy],
                  heading_logical_size=[(heading[2] - heading[0]) / sx, (heading[3] - heading[1]) / sy],
                  button_logical_origin=[button[0] / sx, button[1] / sy])
    return result, image.crop(heading)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    report = json.loads((args.directory / 'report.json').read_text())
    output = args.directory / 'pixel-metrics.json'
    assert not output.exists(), 'analysis output already exists'
    results, headings = {}, {}
    for label in ['before', 'density-2x', 'restored']:
        results[label], headings[label] = measure(args.directory / label / 'source.png', report['logical'])
    base, dense = headings['before'], headings['density-2x']
    difference = ImageChops.difference(base.resize(dense.size, Image.Resampling.BICUBIC), dense)
    results['heading_difference_from_resampled_baseline_rgb_mae'] = sum(ImageStat.Stat(difference).mean) / 3
    results['limits'] = 'Fresh offline login UI; pixel bounds, not OCR or a readability score. Resampling is only a comparator.'
    output.write_text(json.dumps(results, indent=2) + '\n')
    print(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
