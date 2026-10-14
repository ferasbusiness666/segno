# Usage: python tools/make_icon.py assets   (needs Pillow: pip install pillow)
# Draws the Segno mark: rose gradient tile with the segno sign (an S, a slash, two dots),
# the music sign for "go back and play again".
from PIL import Image, ImageDraw
import math, sys

WHITE = (255, 255, 255, 255)

def tile(S):
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    grad = Image.new("RGBA", (S, S))
    top, bot = (0xFF, 0x6B, 0x86), (0xD4, 0x1F, 0x4E)
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / (S - 1)
        gd.line([(0, y), (S, y)], fill=tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3)) + (255,))
    mask = Image.new("L", (S, S), 0)
    m = int(S * 0.04)
    ImageDraw.Draw(mask).rounded_rectangle([m, m, S - m, S - m], radius=int(S * 0.24), fill=255)
    img.paste(grad, (0, 0), mask)
    return img

def cap(d, x, y, w):
    d.ellipse([x - w / 2, y - w / 2, x + w / 2, y + w / 2], fill=WHITE)

def draw(size, top_end=305, bot_end=125, rx=0.13, ry=0.11, w_=0.072, slash=0.25, slash_ang=-48, dot=(0.235, 0.045)):
    S = size * 4  # supersample
    img = tile(S)
    d = ImageDraw.Draw(img)
    cx, cy = S / 2, S / 2
    rx, ry, w = S * rx, S * ry, S * w_
    # Upper bowl: from the spine, round the left and over the top, ending upper right.
    d.arc([cx - rx, cy - 2 * ry, cx + rx, cy], start=90, end=top_end, fill=WHITE, width=int(w))
    # Lower bowl: from the spine, round the right and under, ending lower left.
    d.arc([cx - rx, cy, cx + rx, cy + 2 * ry], start=270, end=360 + bot_end, fill=WHITE, width=int(w))
    # Round the stroke ends.
    for (ccy, ang) in ((cy - ry, top_end), (cy + ry, bot_end)):
        a = math.radians(ang)
        cap(d, cx + (rx - w / 2) * math.cos(a), ccy + (ry - w / 2) * math.sin(a), w)
    # The slash, bottom left to top right, through the middle.
    L = S * slash
    a = math.radians(slash_ang)
    x0, y0 = cx - L * math.cos(a), cy - L * math.sin(a)
    x1, y1 = cx + L * math.cos(a), cy + L * math.sin(a)
    sw = w * 0.72
    d.line([(x0, y0), (x1, y1)], fill=WHITE, width=int(sw))
    cap(d, x0, y0, sw)
    cap(d, x1, y1, sw)
    # The two dots, either side of the middle.
    dr = w * 0.6
    for (dx, dy) in ((-dot[0], dot[1]), (dot[0], -dot[1])):
        x, y = cx + S * dx, cy + S * dy
        d.ellipse([x - dr, y - dr, x + dr, y + dr], fill=WHITE)
    return img.resize((size, size), Image.LANCZOS)

if __name__ == "__main__":
    out = sys.argv[1]
    sizes = [16, 24, 32, 48, 64, 128, 256]
    imgs = [draw(s) for s in sizes]
    imgs[-1].save(out + "/icon_256.png")
    imgs[2].save(out + "/icon_32.png")
    imgs[-1].save(out + "/icon.ico", sizes=[(s, s) for s in sizes])
    if len(sys.argv) > 2:
        prev = Image.new("RGBA", (512 + 16 + 64 + 16 + 32 + 16 + 16, 512), (18, 18, 21, 255))
        x = 0
        for s in (512, 64, 32, 16):
            prev.paste(draw(s), (x, (512 - s) // 2), draw(s))
            x += s + 16
        prev.save(sys.argv[2])
