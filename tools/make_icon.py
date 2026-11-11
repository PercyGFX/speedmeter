"""Generates assets/app.ico, assets/icon_64.rgba and assets/icon_256.png.

The mark is a speed gauge: an arc that runs from calm blue through amber to red
(the app's speed colors) with a needle well into the fast end, on a dark tile.

Run: python tools/make_icon.py
"""
import math
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

TOP = (46, 51, 64)
BOTTOM = (17, 19, 25)
# Along the arc, start to end.
STOPS = [(0.00, (56, 189, 248)), (0.36, (52, 211, 153)), (0.68, (251, 191, 36)), (1.00, (248, 113, 113))]
START, SWEEP = 135.0, 270.0  # degrees, clockwise from 3 o'clock; the gap is at the bottom
NEEDLE_AT = 0.74


def lerp(a, b, t):
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def arc_color(t: float):
    for (t0, c0), (t1, c1) in zip(STOPS, STOPS[1:]):
        if t <= t1:
            return lerp(c0, c1, (t - t0) / (t1 - t0))
    return STOPS[-1][1]


def tile(s: int, radius: int) -> Image.Image:
    """The rounded tile: a soft top-to-bottom gradient with a lit top edge."""
    grad = Image.new("RGBA", (s, s))
    px = ImageDraw.Draw(grad)
    for y in range(s):
        px.line([(0, y), (s, y)], fill=lerp(TOP, BOTTOM, (y / (s - 1)) ** 0.9) + (255,))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, s - 1, s - 1], radius=radius, fill=255)
    out = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    out.paste(grad, (0, 0), mask)
    return out


def gauge(s: int, thick: float, glow: bool) -> Image.Image:
    layer = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    cx, cy, r = s * 0.5, s * 0.565, s * 0.335
    box = [cx - r, cy - r, cx + r, cy + r]
    w = round(s * thick)
    steps = 180
    for i in range(steps):
        a0 = START + SWEEP * i / steps
        a1 = START + SWEEP * (i + 1) / steps + 0.6  # slight overlap hides seams
        d.arc(box, a0, a1, fill=arc_color((i + 0.5) / steps) + (255,), width=w)
    # Round ends.
    for t in (0.0, 1.0):
        a = math.radians(START + SWEEP * t)
        ex, ey = cx + (r - w / 2) * math.cos(a), cy + (r - w / 2) * math.sin(a)
        d.ellipse([ex - w / 2, ey - w / 2, ex + w / 2, ey + w / 2], fill=arc_color(t) + (255,))

    # Needle: a tapered pointer from the hub toward the arc.
    a = math.radians(START + SWEEP * NEEDLE_AT)
    tip = (cx + (r - w * 1.05) * math.cos(a), cy + (r - w * 1.05) * math.sin(a))
    half = s * (0.03 if thick < 0.1 else 0.046)
    nx, ny = -math.sin(a) * half, math.cos(a) * half
    back = (cx - math.cos(a) * half * 1.4, cy - math.sin(a) * half * 1.4)
    white = (245, 247, 250, 255)
    d.polygon([(back[0] + nx, back[1] + ny), tip, (back[0] - nx, back[1] - ny)], fill=white)
    hub = half * (1.7 if thick < 0.1 else 1.25)
    d.ellipse([cx - hub, cy - hub, cx + hub, cy + hub], fill=white)
    if thick < 0.1:
        dot = hub * 0.42
        d.ellipse([cx - dot, cy - dot, cx + dot, cy + dot], fill=BOTTOM + (255,))

    if not glow:
        return layer
    halo = layer.filter(ImageFilter.GaussianBlur(s * 0.035))
    halo.putalpha(halo.getchannel("A").point(lambda v: int(v * 0.45)))
    return Image.alpha_composite(halo, layer)


def draw(size: int) -> Image.Image:
    ss = 8  # supersample factor
    s = size * ss
    radius = round(s * 0.225)
    img = tile(s, radius)
    small = size <= 24
    art = gauge(s, 0.125 if small else 0.088, glow=not small)
    # Keep the glow inside the tile.
    art.putalpha(ImageChops.multiply(art.getchannel("A"), img.getchannel("A")))
    img = Image.alpha_composite(img, art)
    if size >= 32:
        # A hairline that's brighter at the top, like light catching the edge.
        edge = Image.new("RGBA", (s, s), (0, 0, 0, 0))
        ImageDraw.Draw(edge).rounded_rectangle([ss // 2, ss // 2, s - 1 - ss // 2, s - 1 - ss // 2], radius=radius, outline=(255, 255, 255, 255), width=ss)
        fade = Image.linear_gradient("L").resize((s, s)).point(lambda v: int(60 - v * 0.19))
        edge.putalpha(ImageChops.multiply(edge.getchannel("A"), fade))
        img = Image.alpha_composite(img, edge)
    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    ASSETS.mkdir(exist_ok=True)
    sizes = [256, 64, 48, 40, 32, 24, 20, 16]
    images = [draw(sz) for sz in sizes]
    images[0].save(
        ASSETS / "app.ico",
        format="ICO",
        sizes=[(sz, sz) for sz in sizes],
        append_images=images[1:],
    )
    (ASSETS / "icon_64.rgba").write_bytes(draw(64).tobytes())
    draw(256).save(ASSETS / "icon_256.png")
    print("wrote", ASSETS / "app.ico")


if __name__ == "__main__":
    main()
