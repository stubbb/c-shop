# Driving C-Shop without a window

A command language for callers that cannot see and cannot click — an agent, a
batch job, a test. Intake, draw, analyse, return.

```sh
cshop --script build.txt          # run a file
cshop --run "new 400 300 ..."     # run commands inline
cshop --script build.txt --json   # machine-readable report
```

Exit status is `0` when every step succeeded and `2` when any failed, so a
caller can branch on it without parsing anything.

It drives the same application the buttons do. There is no second API to drift
out of step: anything the editor gains is reachable here the same day.

## The loop it is built for

A script goes in, the document is built, and a **report** comes out saying what
actually happened — where every layer landed, what each step did, and what
failed and why — alongside the rendered image. A caller that cannot see the
canvas can still tell whether the text fitted or the shadow fell off the edge.

```
$ cshop --run 'new 400 240 background=#20304a
text 40 154 "Hello, agent" size=54 color=#ffffff bold
effect drop-shadow distance=6 size=8
export out.png'

Untitled: 400x240, 2 layers
  [0] Background               Raster  at (0, 0) 400x240
  [1] Hello, agent             Type    at (1, 71) 397x132  fx: Drop Shadow
5 steps ran, 0 failed
```

Two rules follow from the design:

- **Nothing fails silently.** An unknown command, an unparseable value, an
  action that could not apply — each becomes a failed step with a reason, and
  the run carries on so one typo does not discard the rest.
- **Measurements are free.** `measure` reports the size of type without drawing
  it, so a caller can place something before committing rather than rendering
  and guessing.

Placing text centred, without seeing it:

```
measure text "Hello, agent" size=54 bold
  → measure "Hello, agent": 319x54 (offset 0, -44)
```

319 wide on a 400 canvas puts the left edge at `(400-319)/2 = 40`; the offset
says the baseline sits 44 below the ink's top, so a baseline of `154` puts the
text's top at 110.

## Syntax

One command per line. `#` starts a comment. Arguments are positional, options
are `key=value`, and strings are double-quoted with `\n`, `\t` and `\"`
escapes. A bare word like `bold` means `bold=true` — nothing tells the parser
it is not a positional argument, so **bare flags go last**.

Colours are `#rgb`, `#rrggbb`, `#rrggbbaa`, or one of `black white red green
blue yellow orange purple grey transparent`. Paths may start with `~`.

## Commands

| Command | What it does |
|---|---|
| `new W H [background=]` | Start a document. Background is `white`, `transparent` or a colour. |
| `open PATH` | Open an image, a `.psd` or a `.cshop` project, with its layers. |
| `text X Y "..."` | A type layer, its baseline starting at X Y. `size= color= family= bold italic align= leading= tracking= wrap=` |
| `measure text "..."` | Report the size the same options would draw, without drawing it. |
| `shape KIND X Y W H` | `rect ellipse polygon star line`. `fill= stroke= stroke-width= stroke-align= radius= sides= inner= thickness=` |
| `fill COLOUR` | Fill the layer, or the selection if there is one. |
| `gradient X1 Y1 X2 Y2` | A gradient across the layer. Colours carry alpha, so `from=#00000000 to=#000000cc` is a wash that fades out. `style= blend= opacity= reverse` |
| `select X Y W H` \| `select all` \| `select none` | `feather=` softens the edge. |
| `effect NAME` | See below. Applies to the active layer; repeat to stack. |
| `filter NAME` | `gaussian-blur box-blur motion-blur sharpen unsharp-mask add-noise high-pass find-edges median mosaic` |
| `adjust NAME` | `brightness-contrast hue-saturation vibrance exposure invert posterize threshold black-and-white`, plus `as-layer` to keep it editable. |
| `layer WHAT` | `new group duplicate delete merge-down flatten rasterize select <index>` |
| `set key=value` | `opacity= fill-opacity= name= blend=` on the active layer. |
| `move DX DY` | Nudge the active layer. |
| `order WHERE` | `top bottom up down` |
| `info` | Report the document's size and layer count. |
| `export PATH` | Write it. The extension decides: `.cshop` and `.psd` keep layers, everything else is flattened. |

### Effects

`drop-shadow inner-shadow outer-glow inner-glow bevel emboss satin
color-overlay gradient-overlay pattern-overlay stroke`, and `none` to clear
them.

Common options: `color= opacity= size= blend=`. Then by effect:
`distance= angle= spread=` or `choke=` for shadows and glows; `position=` for
stroke (`inside centre outside`); `from= to= style= scale= reverse` for the
gradient; `pattern= background= scale= angle=` for the pattern; `style= depth=
soften= altitude=` for the bevel.

## Worked example

```
open portrait.jpg
measure text "SUMMER" size=120 bold

text 80 400 "SUMMER" size=120 color=#ffffff bold tracking=60
effect drop-shadow distance=10 size=14 opacity=0.55
effect stroke size=3 color=#1a1a1a position=outside

shape rect 60 430 520 6 fill=#ffcc33
adjust vibrance vibrance=0.25 as-layer
export poster.png
```

## What it does not do

No loops, variables or arithmetic — a caller that needs those has a real
language and can emit the script. Curves and levels take no parameters here
yet, and there is no way to paint a brush stroke: the pathway is aimed at
composition, type and effects rather than freehand work.
