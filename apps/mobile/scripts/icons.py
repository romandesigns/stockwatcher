"""Render the phone app's icons from the one stockwatcher mark.

The mark is apps/web/public/favicon.svg (the desktop app's icons are the same drawing): a rising
line #8b9cff on #0f1216. Owner, 2026-10-22: reuse that icon for everything stockwatcher,
including the Expo app, which still had Expo's template icons. Run from apps/mobile:
`python scripts/icons.py` (Pillow), then rebuild: icons are native, an OTA update cannot change them.
"""
from PIL import Image, ImageDraw

BG = (0x0F, 0x12, 0x16, 255)
LINE = (0x8B, 0x9C, 0xFF, 255)
WHITE = (255, 255, 255, 255)
PTS = [(6, 21), (12, 15), (17, 19), (26, 10)]  # favicon.svg path, 32-unit grid, stroke 3


def draw(size, scale, color, bg=None):
    s = 4  # supersample, then downscale for clean edges
    w = size * s
    im = Image.new("RGBA", (w, w), bg or (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    pts = [((x - 16) * scale * s + w / 2, (y - 15.5) * scale * s + w / 2) for x, y in PTS]
    sw = 3 * scale * s
    d.line(pts, fill=color, width=round(sw))
    for x, y in pts:  # round caps and joins, as in the SVG
        d.ellipse([x - sw / 2, y - sw / 2, x + sw / 2, y + sw / 2], fill=color)
    return im.resize((size, size), Image.LANCZOS)


# iOS and the legacy Android icon: full-bleed, the OS applies its own mask (no transparency on iOS).
draw(1024, 32, LINE, BG).convert("RGB").save("assets/icon.png")
# Adaptive icon: the line stays inside the central 66% safe zone at scale 24.
draw(1024, 24, LINE).save("assets/android-icon-foreground.png")
Image.new("RGB", (1024, 1024), BG[:3]).save("assets/android-icon-background.png")
# Themed icon and the notification's status-bar silhouette (Android uses only its alpha).
draw(1024, 24, WHITE).save("assets/android-icon-monochrome.png")
draw(1024, 40, LINE).save("assets/splash-icon.png")
