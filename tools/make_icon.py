"""Generates assets/app.ico and assets/icon_64.rgba.

The mark is the app's core idea in miniature: a wire, download bars above it,
upload bars below it, coloured by how heavy the traffic is.

Run: python tools/make_icon.py
"""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

BG = (32, 32, 32, 255)
BORDER = (64, 64, 64, 255)
WIRE = (150, 150, 150, 255)
CALM = (120, 134, 152, 255)
BLUE = (76, 194, 255, 255)
AMBER = (255, 200, 61, 255)
RED = (255, 107, 94, 255)


def draw(size: int) -> Image.Image:
    ss = 8  # supersample factor
    s = size * ss
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    radius = round(s * 0.24)
    d.rounded_rectangle([0, 0, s - 1, s - 1], radius=radius, fill=BG)
    if size >= 32:
        d.rounded_rectangle([0, 0, s - 1, s - 1], radius=radius, outline=BORDER, width=ss)

    wire_y = round(s * 0.60)
    n = 4 if size <= 20 else 5
    left, right = s * 0.19, s * 0.81
    slot = (right - left) / n
    bar_w = slot * 0.62
    ups = [0.30, 0.52, 0.42, 0.20, 0.34][:n] if n == 5 else [0.30, 0.52, 0.40, 0.22]
    downs = [0.10, 0.18, 0.14, 0.08, 0.12][:n] if n == 5 else [0.10, 0.18, 0.13, 0.08]
    colors = [BLUE, RED, AMBER, CALM, BLUE][:n] if n == 5 else [BLUE, RED, AMBER, CALM]

    for i in range(n):
        x0 = left + i * slot + (slot - bar_w) / 2
        x1 = x0 + bar_w
        top = wire_y - ups[i] * s
        bottom = wire_y + downs[i] * s
        r = bar_w / 2
        d.rounded_rectangle([x0, top, x1, wire_y], radius=r, fill=colors[i])
        dim = tuple(int(c * 0.72) for c in colors[i][:3]) + (255,)
        d.rounded_rectangle([x0, wire_y, x1, bottom], radius=r, fill=dim)

    wire_h = max(ss, round(s / 28))
    d.rectangle([s * 0.12, wire_y - wire_h / 2, s * 0.88, wire_y + wire_h / 2], fill=WIRE)
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
