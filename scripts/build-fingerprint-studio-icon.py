#!/usr/bin/env python3
"""Render the checked-in fingerprint vectors into native macOS app icons.

Requires ImageMagick's `magick` and macOS `iconutil`. The .png.bin files are PNG
images with a source-friendly suffix (this repository ignores capture *.png).
They can be decoded from include_bytes! by GPUI without a runtime asset path.
"""
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "apps" / "fingerprint-studio" / "assets"
# Deliberately uneven ridge flow gives the mark a fingerprint silhouette rather
# than the straight-sided U/arch motif used by the first icon.
PATHS = [
    "M 201,587 C 171,479 190,363 255,278",
    "M 293,226 C 399,142 548,139 658,197 C 787,267 839,408 800,556",
    "M 243,665 C 192,522 222,358 333,277 C 432,206 554,208 649,267 C 742,325 780,432 750,543 C 732,613 737,685 765,741",
    "M 285,727 C 225,593 239,437 326,343 C 406,256 532,249 625,309 C 710,363 733,451 705,545 C 682,625 691,721 725,786",
    "M 331,775 C 282,668 272,535 309,433 C 349,324 465,280 563,327 C 649,368 682,449 657,532 C 625,639 641,747 680,815",
    "M 382,815 C 331,711 322,591 341,489 C 357,399 414,353 481,351 C 568,347 622,410 610,486 C 593,582 583,683 626,786",
    "M 434,833 C 391,743 368,648 381,540 C 389,461 403,409 458,399 C 504,390 551,419 558,465 C 564,509 542,554 534,602 C 522,686 534,774 577,827",
    "M 480,831 C 443,750 426,676 431,590 C 434,533 454,498 455,461",
    "M 500,451 C 511,493 497,539 486,581 C 467,651 483,741 517,802",
]
SMALL_PATHS = [PATHS[index] for index in (1, 2, 4, 6, 8)]
TILE = "#e8eef8"
TILE_BORDER = "#d5dfed"
INK = "#245ac6"
STROKE = 26



def svg(background: bool) -> str:
    backdrop = '<rect x="64" y="64" width="896" height="896" rx="202" fill="#e8eef8" stroke="#d5dfed" stroke-width="2"/>' if background else ""
    color = INK
    paths = "\n".join(f'<path d="{path}" fill="none" stroke="{color}" stroke-width="26" stroke-linecap="round" stroke-linejoin="round"/>' for path in PATHS)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
<title>Fingerprint Studio</title>
{backdrop}
<g fill="none" stroke="{color}" stroke-width="26" stroke-linecap="round" stroke-linejoin="round">
{paths}
</g>
</svg>
'''


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def main() -> None:
    for name in ("magick", "iconutil"):
        if shutil.which(name) is None:
            raise SystemExit(f"Required icon build tool is unavailable: {name}")
    ASSETS.mkdir(parents=True, exist_ok=True)
    app_svg = ASSETS / "fingerprint-studio.svg"
    mark_svg = ASSETS / "fingerprint-mark.svg"
    app_svg.write_text(svg(True), encoding="utf-8")
    mark_svg.write_text(svg(False), encoding="utf-8")
    with tempfile.TemporaryDirectory(prefix="fingerprint-icon-") as scratch:
        iconset = Path(scratch) / "FingerprintStudio.iconset"
        iconset.mkdir()
        # Use the shared vector paths directly: ImageMagick's built-in SVG
        # reader on some Macs silently drops SVG path strokes.
        masters = []
        for background, name in ((True, "fingerprint-studio"), (False, "fingerprint-mark")):
            master = Path(scratch) / f"{name}.png"
            args = ["magick", "-size", "1024x1024", "xc:none"]
            if background:
                args += ["-fill", TILE, "-stroke", TILE_BORDER, "-strokewidth", "2", "-draw", "roundrectangle 64,64 960,960 202,202"]
            args += ["-fill", "none", "-stroke", INK, "-strokewidth", str(STROKE)]
            for path in PATHS:
                args += ["-draw", f"stroke-linecap round stroke-linejoin round path '{path}'"]
            args += ["-strip", f"PNG:{master}"]
            run(*args)
            run("magick", str(master), "-resize", "256x256", "-depth", "8", "-strip", f"PNG:{ASSETS / (name + '.png.bin')}")
            masters.append(master)
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = size * scale
                name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
                source = masters[0]
                if pixels <= 32:
                    # Fewer, slightly heavier ridges survive small Dock/Finder
                    # rendering. Render from vectors rather than sharpen a blur.
                    source = Path(scratch) / "small-master.png"
                    args = ["magick", "-size", "1024x1024", "xc:none", "-fill", TILE,
                            "-stroke", TILE_BORDER, "-strokewidth", "2", "-draw",
                            "roundrectangle 64,64 960,960 202,202", "-fill", "none",
                            "-stroke", INK, "-strokewidth", "48" if pixels <= 16 else "38"]
                    paths = [PATHS[index] for index in (2, 5, 8)] if pixels <= 16 else SMALL_PATHS
                    for path in paths:
                        args += ["-draw", f"stroke-linecap round stroke-linejoin round path '{path}'"]
                    run(*args, "-strip", f"PNG:{source}")
                run("magick", str(source), "-resize", f"{pixels}x{pixels}", "-depth", "8", "-strip", f"PNG:{iconset / name}")
        run("iconutil", "-c", "icns", str(iconset), "-o", str(ASSETS / "FingerprintStudio.icns"))
    print(f"Generated Fingerprint Studio SVG, embedded PNG, and ICNS assets in {ASSETS}")


if __name__ == "__main__":
    main()
