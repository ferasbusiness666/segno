# Segno logo

The mark is a record seen from above: a solid label with a play mark, and two curved lines of
shine across from each other. The shine is what makes it a spinning record, not just a play
button in a circle, so keep it in every version except the small cut.

## Files

| File | Use it for |
|---|---|
| `segno-app-icon.svg` | The app icon: the pink record on its own, no tile |
| `segno-app-icon-small.svg` | The same at 16–24 px (no shine, bigger play mark): taskbar, tray, favicon |
| `segno-mark-line-*.svg` | Website header, social, merch, anywhere it's shown big |
| `segno-mark-solid-*.svg` | Small sizes and busy backgrounds |
| `segno-logo-*.svg` | The mark with the name ("Segno"), side by side |
| `png/` | PNG copies of all of the above |
| `web/` | Favicon, touch icon, manifest icons and the `<head>` lines for the website |

Every file comes in **black** (`#111114`), **white** and **pink** (`#FF4D6D`). The logo files:
`black` and `pink` for light backgrounds, `white` and `pink-on-dark` for dark ones.

## Colours

| Name | HEX | RGB |
|---|---|---|
| Segno Pink | `#FF4D6D` | 255 77 109 |
| Ink | `#111114` | 17 17 20 |
| White | `#FFFFFF` | 255 255 255 |

## Rules

- **Space around it:** keep at least the size of the label (about a fifth of the mark's width)
  clear on every side.
- **Smallest size:** the line and solid marks down to 24 px; below that use the small cut.
- **Don't** stretch it, rotate it, add shadows or gradients, change the colours, outline it, or
  put it on a busy photo without a solid tile behind it.
- The name is set in **Inter Display Bold** (SIL Open Font License, free to use in a logo) and
  already turned into shapes in the SVGs, so the font doesn't need to be installed.

## Changing it

Everything is drawn in `make_brand.py` from circles, arcs and one rounded triangle. Change the
numbers there, then:

```
pip install cairosvg pillow fonttools
python brand/make_brand.py
python brand/export_icons.py
```

`export_icons.py` also writes the app's own icon files in `assets/` (the exe icon, the window
icon, the in-app logo and the tray icon).
