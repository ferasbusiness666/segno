"""Segno brand files: the Record mark, drawn with exact circles and arcs.

    python brand/make_brand.py            (writes the SVGs into brand/)

The mark: a record seen from above. A solid label with a play mark in the middle, and two
curved lines of shine across from each other (what makes it a spinning record and not just a
play button in a circle).

Every shape is a filled path (no strokes), built from circles, circular arcs and one rounded
triangle, on a 256 x 256 canvas. Overlaps use the even-odd rule, so holes cut through.
"""
import math
import os

HERE = os.path.dirname(os.path.abspath(__file__))
C = 128.0

PINK = "#FF4D6D"
INK = "#111114"
WHITE = "#FFFFFF"


def f(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return s if s != "-0" else "0"


def circle(r, cx=C, cy=C):
    """A full circle as two arcs."""
    return (f"M{f(cx - r)} {f(cy)}A{f(r)} {f(r)} 0 1 0 {f(cx + r)} {f(cy)}"
            f"A{f(r)} {f(r)} 0 1 0 {f(cx - r)} {f(cy)}Z")


def pt(r, deg, cx=C, cy=C):
    a = math.radians(deg)
    return cx + r * math.cos(a), cy + r * math.sin(a)


def arc_band(r, w, a0, a1, cx=C, cy=C):
    """A curved line with round ends: the band between radius r-w/2 and r+w/2 from angle a0 to
    a1 (degrees, clockwise on screen), closed by two half circles."""
    ro, ri, cap = r + w / 2, r - w / 2, w / 2
    large = 1 if (a1 - a0) % 360 > 180 else 0
    x0, y0 = pt(ro, a0, cx, cy)
    x1, y1 = pt(ro, a1, cx, cy)
    x2, y2 = pt(ri, a1, cx, cy)
    x3, y3 = pt(ri, a0, cx, cy)
    return (f"M{f(x0)} {f(y0)}A{f(ro)} {f(ro)} 0 {large} 1 {f(x1)} {f(y1)}"
            f"A{f(cap)} {f(cap)} 0 0 1 {f(x2)} {f(y2)}"
            f"A{f(ri)} {f(ri)} 0 {large} 0 {f(x3)} {f(y3)}"
            f"A{f(cap)} {f(cap)} 0 0 1 {f(x0)} {f(y0)}Z")


def play(h, corner, cx=C, cy=C):
    """An equilateral play triangle of height h pointing right, corners rounded by `corner`,
    centred on its centroid (which is what looks centred inside a circle)."""
    side = h * 2 / math.sqrt(3)
    # Sharp corners, centroid at (cx, cy).
    p = [(cx - h / 3, cy - side / 2), (cx + 2 * h / 3, cy), (cx - h / 3, cy + side / 2)]
    # Each rounded corner: a circle of radius `corner` tangent to both sides, inside the corner.
    # For an equilateral triangle the corner angle is 60°, so the circle's centre sits
    # corner / sin(30°) = 2 * corner from the vertex, towards the centroid.
    out = []
    for i in range(3):
        vx, vy = p[i]
        dx, dy = cx - vx, cy - vy
        d = math.hypot(dx, dy)
        ccx, ccy = vx + dx / d * 2 * corner, vy + dy / d * 2 * corner
        # Tangent points on the two sides meeting at this vertex.
        tangents = []
        for j in (i - 1, i + 1):
            ox, oy = p[j % 3]
            ux, uy = ox - vx, oy - vy
            L = math.hypot(ux, uy)
            t = 2 * corner * math.cos(math.radians(30))  # distance from vertex to tangent point
            tangents.append((vx + ux / L * t, vy + uy / L * t))
        out.append((tangents[0], tangents[1], (ccx, ccy)))
    d = f"M{f(out[0][1][0])} {f(out[0][1][1])}"
    for k in range(1, 4):
        a_in, a_out, _ = out[k % 3]
        d += f"L{f(a_in[0])} {f(a_in[1])}A{f(corner)} {f(corner)} 0 0 1 {f(a_out[0])} {f(a_out[1])}"
    return d + "Z"


# ---------------------------------------------------------------------------------------------
# Geometry (256 canvas). One place to tune everything.
SHINE_AT = -58  # where the upper shine is centred (the lower one is opposite)


def solid_mark(R=108, gap_in=46, gap_out=53, tri=40, corner=4, shine_r=78, shine_w=9, span=46, shine=True):
    """Solid record: the disc, a thin ring cut around the label, the play mark cut from the label
    and the shine cut from the vinyl."""
    k = R / 108
    parts = [circle(R), circle(gap_out * k), circle(gap_in * k), play(tri * k, corner * k)]
    if shine:
        parts += [arc_band(shine_r * k, shine_w * k, SHINE_AT - span / 2, SHINE_AT + span / 2),
                  arc_band(shine_r * k, shine_w * k, SHINE_AT + 180 - span / 2, SHINE_AT + 180 + span / 2)]
    return "".join(parts)


def line_mark(R=108, w=12, label=44, tri=38, corner=4, shine_r=74, span=50):
    """Line record: the outer edge as a ring, a solid label with the play mark cut out, and the
    shine as two curved lines of the same weight."""
    return "".join([
        circle(R), circle(R - w),
        circle(label), play(tri, corner),
        arc_band(shine_r, w, SHINE_AT - span / 2, SHINE_AT + span / 2),
        arc_band(shine_r, w, SHINE_AT + 180 - span / 2, SHINE_AT + 180 + span / 2),
    ])


def small_mark(R=120):
    """For 16–24 px: no shine and a bigger play mark, so it stays crisp when tiny."""
    k = R / 108
    return "".join([circle(R), circle(56 * k), circle(47 * k), play(44 * k, 5 * k)])


def svg(paths, title, w=256, h=256, extra=""):
    body = "".join(f'<path fill="{fill}" fill-rule="evenodd" d="{d}"/>' for d, fill in paths)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" '
            f'aria-labelledby="t"><title id="t">{title}</title>{extra}{body}</svg>\n')


# ---------------------------------------------------------------------------------------------
# Wordmark: "Segno" set in Inter Display Bold (SIL Open Font License, fine for logos), outlined.
def wordmark_path(text="Segno", size=100.0, tracking=-0.025):
    from fontTools.ttLib import TTFont
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.transformPen import TransformPen
    font = TTFont(os.path.join(HERE, "..", "assets", "InterDisplay-Bold.otf"))
    gs = font.getGlyphSet()
    cmap = font.getBestCmap()
    k = size / font["head"].unitsPerEm
    pen = SVGPathPen(gs, ntos=lambda v: f"{v:.2f}".rstrip("0").rstrip("."))
    x = 0.0
    for ch in text:
        g = cmap[ord(ch)]
        gs[g].draw(TransformPen(pen, (k, 0, 0, -k, x, 0)))
        x += gs[g].width * k + tracking * size
    width = x - tracking * size
    cap = font["OS/2"].sCapHeight * k
    return pen.getCommands(), width, cap


def lockup(mark_d, mark_fill, word_fill, bg=None):
    """Mark on the left, "Segno" on the right. Height 256: the mark is 256 tall, the capitals
    are 52 % of the mark, the gap is a quarter of the mark."""
    d, width, cap = wordmark_path(size=100)
    size = 100 * (0.52 * 256) / cap
    d, width, cap = wordmark_path(size=size)
    gap = 64
    wx = 256 + gap
    base = C + cap / 2
    W = wx + width + 8
    bgr = f'<rect width="{f(W)}" height="256" fill="{bg}"/>' if bg else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {f(W)} 256" width="{f(W)}" height="256" role="img" '
            f'aria-labelledby="t"><title id="t">Segno</title>{bgr}'
            f'<path fill="{mark_fill}" fill-rule="evenodd" d="{mark_d}"/>'
            f'<path fill="{word_fill}" transform="translate({f(wx)} {f(base)})" d="{d}"/></svg>\n')


def main():
    out = HERE
    marks = {"line": line_mark(), "solid": solid_mark()}
    colours = {"black": INK, "white": WHITE, "pink": PINK}
    for name, d in marks.items():
        for cname, fill in colours.items():
            open(os.path.join(out, f"segno-mark-{name}-{cname}.svg"), "w").write(svg([(d, fill)], "Segno"))
    # App icon: the pink record on its own (Windows icons don't need a tile), and its small cut.
    open(os.path.join(out, "segno-app-icon.svg"), "w").write(svg([(solid_mark(R=120, shine_w=10), PINK)], "Segno"))
    open(os.path.join(out, "segno-app-icon-small.svg"), "w").write(svg([(small_mark(), PINK)], "Segno"))
    # Lockups
    L = {
        "segno-logo-black": (marks["line"], INK, INK, None),
        "segno-logo-white": (marks["line"], WHITE, WHITE, None),
        "segno-logo-pink-on-dark": (marks["line"], PINK, WHITE, None),
        "segno-logo-pink": (marks["line"], PINK, INK, None),
    }
    for name, (d, mf, wf, bg) in L.items():
        open(os.path.join(out, f"{name}.svg"), "w").write(lockup(d, mf, wf, bg))
    print("brand files written to", out)


if __name__ == "__main__":
    main()
