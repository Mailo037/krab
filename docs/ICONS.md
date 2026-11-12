# How Krab's icons are made

Every icon in Krab is drawn by hand as vector geometry in the source code. There are no image files for the
toolbar, no icon font and no third-party icon set: the shapes are compiled into the binary, scale to any display
density and cost nothing to load.

There are two kinds of icons:

| What | Where | Used for |
|---|---|---|
| Toolbar icons | `src/gfx/icons.rs` | the toolbar, mode switcher, colour bar and status windows |
| The Krab mark | `src/brand.rs` | app icon (`assets/icon.ico`, `assets/icon.png`), tray icon, banner |

## Look at them first

```sh
cargo run --example icon_sheet
```

writes `target/icon-sheet.png`: every icon large on its grid (with the safe area outlined in orange), at toolbar
size on a 2x display and on a 1x display, and the Krab mark at app-icon and tray sizes. Keep it open while you
draw and re-run the command after every change.

## The style

All toolbar icons follow the same rules, so a new one fits in without anyone noticing:

| Rule | Value |
|---|---|
| Grid | 24 x 24 units, drawn at 20 logical px in the toolbar |
| Safe area | everything between 2 and 22 on both axes (a test enforces this) |
| Stroke | 1.9 units, round caps, round joins - never thinner than 1.4 device px |
| Corners | always rounded: rectangles 3 - 4 units, bends and arrow heads 1.5 - 2.6 units |
| Ends | open lines end in round caps; no sharp points except where the meaning needs one (pen tip) |
| Fill | only for solid states (stop, record dot) and for contrast inside a pair (pixelate) |
| Colour | none - the icon is drawn in whatever colour the caller passes |
| Detail | as little as still reads at 16 px; check the 1x rendering on the sheet |

The roundness comes from three places: round caps and joins on every stroke, generous corner radii on
rectangles, and the `P` / `G` commands below that round every corner of a polyline automatically.

## The path language

Each icon is a pair of strings - geometry to **stroke** and geometry to **fill** - written in a small path
language. It is SVG path data (absolute coordinates only) plus a few shapes:

| Command | Meaning |
|---|---|
| `M x y` | move to |
| `L x y` | line to; further number pairs are more line-tos |
| `Q cx cy x y` | quadratic curve with control point `cx cy` |
| `C c1x c1y c2x c2y x y` | cubic curve |
| `Z` | close the current figure |
| `O cx cy r` | circle |
| `R x y w h r` | rectangle with corner radius `r` |
| `P r x y x y ...` | open polyline through the points, every inner corner rounded by `r` |
| `G r x y x y ...` | closed polygon through the points, every corner rounded by `r` |

Commands may touch their first number (`M6 3.5`) and several figures go into one string.

`P` and `G` do the rounding for you: each corner is replaced by a curve that starts `r` units before the corner
and ends `r` units after it. On short edges the radius shrinks so that neighbouring corners never overlap, and
the free ends of a `P` polyline stay exactly where you put them.

### Examples

```text
Arrow   "M5 19 L18.4 5.6  P 2.6 10.5 5 19 5 19 13.5"
         shaft            head: an open polyline with a soft tip

Video   "R 3 6.5 12.5 11 3.6  G 1.6 15.5 10.6 21 7.6 21 16.4 15.5 13.4"
         body: rounded rect   lens: a rounded quadrilateral

Stop    stroke: ""   fill: "R 6.5 6.5 11 11 3.6"
```

## Drawing a new icon, step by step

1. **Sketch on the grid.** Graph paper with a 24 x 24 square works, as does any vector editor with a 24 x 24
   artboard (Figma, Inkscape, Illustrator). Keep to the safe area and think in strokes, not outlines: you draw
   the centre line, the 1.9-unit stroke is added afterwards.
2. **Reduce it to simple shapes.** Most icons are one or two rounded rectangles, circles or polylines plus a
   few straight lines. If you need more than five or six pieces, the icon is probably too detailed.
3. **Write the geometry.** Read the corner coordinates off your sketch and write them with `R`, `O`, `P` and
   `G` first; reach for `Q` / `C` only for real curves. From a vector editor you can copy the path data of a
   stroked shape (with absolute coordinates) and paste it as is.
4. **Add it to the set** in `src/gfx/icons.rs`:
   - a variant in `enum Icon` (before `Folder`, or move the `COUNT` line to the new last variant),
   - the same variant in the `ALL` array, in the same order,
   - its `(stroke, fill)` strings in `source()`.
5. **Look at it**: `cargo run --example icon_sheet`. Compare it with its neighbours at 1x - weight and size
   should match, nothing should look sharper than the rest.
6. **Test**: `cargo test icons` checks that every icon parses, has geometry and stays in the safe area.
7. **Use it**: `icons::draw(canvas, Icon::YourIcon, center, size, colour)`.

## The Krab mark (app and tray icon)

The crab is drawn in `src/brand.rs` on a 100 x 100 grid from a handful of shapes: an oval shell, two claws
(discs with a wedge cut out, outlined in the same colour so their tips are round), round-capped lines for arms,
legs and eye stalks, and circles for the eyes.

- `draw_mark(canvas, rect, badge: true)` puts it on a dark *squircle* tile - a superellipse, rounder than a
  rounded rectangle - with four rounded selection corners. This is the app icon.
- `badge: false` draws the crab alone; it is used for the tray and for icon sizes below 32 px, where the tile
  would leave too little room.

After changing it, regenerate the icon files and every picture that shows it:

```sh
cargo run --release --example gen_assets
```

This writes `assets/icon.ico` (16 - 256 px; the Windows executable embeds it at build time),
`assets/icon.png` (512 px, shipped with the Linux download) and the README images.
