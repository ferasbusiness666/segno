"""Render the brand SVGs into the files the app and the website use.

    pip install cairosvg pillow fonttools
    python brand/make_brand.py && python brand/export_icons.py

Writes:
  assets/icon.ico, assets/icon_256.png, assets/icon_32.png   (the app: exe, window and logo; tray)
  brand/png/...                                               (PNG copies of every logo)
  brand/web/...                                               (favicon, touch icon, manifest icons)
Sizes up to 24 px use the small cut (no shine, bigger play mark) so they stay crisp.
"""
import io
import os

import cairosvg
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ASSETS = os.path.join(ROOT, "assets")
PNG = os.path.join(HERE, "png")
WEB = os.path.join(HERE, "web")
SMALL_MAX = 24


def render(svg_name, w, h=None):
    data = cairosvg.svg2png(url=os.path.join(HERE, svg_name), output_width=w, output_height=h or w)
    return Image.open(io.BytesIO(data)).convert("RGBA")


def icon(size):
    return render("segno-app-icon-small.svg" if size <= SMALL_MAX else "segno-app-icon.svg", size)


def on_tile(size, bg="#111114", scale=0.78):
    """The icon centred on a solid square (for places that don't allow transparency)."""
    tile = Image.new("RGBA", (size, size), bg)
    inner = icon(int(size * scale))
    off = (size - inner.width) // 2
    tile.alpha_composite(inner, (off, off))
    return tile


def main():
    os.makedirs(PNG, exist_ok=True)
    os.makedirs(WEB, exist_ok=True)

    # The app
    sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
    imgs = [icon(s) for s in sizes]
    imgs[-1].save(os.path.join(ASSETS, "icon.ico"), sizes=[(s, s) for s in sizes], append_images=imgs[:-1])
    icon(256).save(os.path.join(ASSETS, "icon_256.png"))
    # The tray shows the icon at 16–24 px, so it gets the small cut (rendered at 32 for high DPI).
    render("segno-app-icon-small.svg", 32).save(os.path.join(ASSETS, "icon_32.png"))

    # PNG copies
    for s in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
        icon(s).save(os.path.join(PNG, f"segno-app-icon-{s}.png"))
    for name in os.listdir(HERE):
        if name.startswith("segno-mark-") and name.endswith(".svg"):
            render(name, 1024).save(os.path.join(PNG, name.replace(".svg", ".png")))
        if name.startswith("segno-logo-") and name.endswith(".svg"):
            # Lockups keep their shape: height 256 in the SVG, exported 400 tall.
            import re
            vb = re.search(r'viewBox="0 0 ([\d.]+) 256"', open(os.path.join(HERE, name)).read())
            w = float(vb.group(1)) if vb else 1000
            render(name, int(w * 400 / 256), 400).save(os.path.join(PNG, name.replace(".svg", ".png")))

    # The website
    fav = [icon(s) for s in (16, 32, 48)]
    fav[-1].save(os.path.join(WEB, "favicon.ico"), sizes=[(16, 16), (32, 32), (48, 48)], append_images=fav[:-1])
    with open(os.path.join(HERE, "segno-app-icon.svg")) as src, open(os.path.join(WEB, "favicon.svg"), "w") as dst:
        dst.write(src.read())
    on_tile(180).save(os.path.join(WEB, "apple-touch-icon.png"))
    icon(192).save(os.path.join(WEB, "icon-192.png"))
    icon(512).save(os.path.join(WEB, "icon-512.png"))
    on_tile(512, scale=0.62).save(os.path.join(WEB, "icon-maskable-512.png"))
    with open(os.path.join(WEB, "site.webmanifest"), "w") as fh:
        fh.write("""{
  "name": "Segno",
  "short_name": "Segno",
  "icons": [
    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png" },
    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png" },
    { "src": "/icon-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
  ],
  "theme_color": "#111114",
  "background_color": "#111114",
  "display": "standalone"
}
""")
    with open(os.path.join(WEB, "head.html"), "w") as fh:
        fh.write("""<!-- Put these files in the website's root folder, and this in <head>: -->
<link rel="icon" href="/favicon.ico" sizes="48x48">
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<link rel="manifest" href="/site.webmanifest">
<meta name="theme-color" content="#111114">
""")
    print("icons written")


if __name__ == "__main__":
    main()
