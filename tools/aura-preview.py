#!/usr/bin/env python3
"""Pictures of the sidebar aura (V), one per theme, without a terminal.

    SPOTLITE_PREVIEW=/tmp/aura cargo test aura_preview -- --ignored
    python3 tools/aura-preview.py /tmp/aura out.png [font.ttf]

The test writes each theme's sidebar as text (cells and colours); this draws them side by side.
"""
import glob, os, sys
from PIL import Image, ImageDraw, ImageFont

src, out = sys.argv[1], sys.argv[2]
font_path = sys.argv[3] if len(sys.argv) > 3 else "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Regular.ttf"
CW, CH = 12, 24
font = ImageFont.truetype(font_path, 19)


def braille(d, x, y, code, fg):
    bits = code - 0x2800
    for i, (dx, dy) in enumerate([(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)]):
        if bits >> i & 1:
            cx, cy = x + 3 + dx * 6, y + 3 + dy * 6
            d.ellipse([cx - 1.6, cy - 1.6, cx + 1.6, cy + 1.6], fill=fg)


def panel(path):
    lines = open(path, encoding="utf-8").read().split("\n")
    w, h, ox, oy, name = lines[0].split(" ", 4)
    w, h, ox, oy = int(w), int(h), int(ox), int(oy)
    img = Image.new("RGB", (w * CW, h * CH + 30), (12, 12, 16))
    d = ImageDraw.Draw(img)
    cells = []
    for line in lines[1:]:
        if line.startswith("cover "):
            cover = [int(v) for v in line.split()[1:]]
        elif line.startswith("bg "):
            img.paste(tuple(int(v) for v in line.split()[1:]), [0, 0, w * CW, h * CH])
        elif line:
            p = line.split(" ", 8)
            cells.append(([int(v) for v in p[:8]], p[8]))
    # The cover: the same warm sky over a cold sea the test gives the aura.
    cx, cy, cw, ch = cover
    pic = Image.new("RGB", (8, 8))
    for i in range(64):
        u, v = (i % 8) / 7, (i // 8) / 7
        sky = (250 - 60 * u, 120 + 90 * u * (1 - v), 60 + 120 * v)
        sea = (20 + 40 * u, 110 + 80 * (1 - u), 190 - 40 * v)
        pic.putpixel((i % 8, i // 8), tuple(int(c) for c in (sky if v < 0.55 else sea)))
    img.paste(pic.resize((cw * CW, ch * CH), Image.BICUBIC), ((cx - ox) * CW, (cy - oy) * CH))
    for (x, y, r, g, b, br, bg_, bb), sym in cells:
        px, py = (x - ox) * CW, (y - oy) * CH
        if cx <= x < cx + cw and cy <= y < cy + ch:
            continue
        if br >= 0:
            d.rectangle([px, py, px + CW - 1, py + CH - 1], fill=(br, bg_, bb))
        if 0x2800 <= ord(sym[0]) <= 0x28FF:
            braille(d, px, py, ord(sym[0]), (r, g, b))
        else:
            d.text((px, py), sym, font=font, fill=(r, g, b))
    d.text((6, h * CH + 4), os.path.basename(path)[:-4], font=font, fill=(200, 200, 200))
    return img


shots = [panel(p) for p in sorted(glob.glob(os.path.join(src, "*.txt")))]
per_row = int(os.environ.get("PER_ROW", "4"))
rows = [shots[i:i + per_row] for i in range(0, len(shots), per_row)]
W = max(sum(s.width + 10 for s in r) for r in rows)
H = sum(max(s.height for s in r) + 10 for r in rows)
sheet = Image.new("RGB", (W, H), (0, 0, 0))
y = 0
for r in rows:
    x = 0
    for s in r:
        sheet.paste(s, (x, y))
        x += s.width + 10
    y += max(s.height for s in r) + 10
sheet.save(out)
print(out, sheet.size)
