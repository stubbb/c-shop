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

## Worked example: an agent doing a real job

The point of the report and of `measure` is that they let a caller work
*without seeing the picture*. Here is what that looks like in practice. The
instruction was:

> pick `~/assets/gardenpath.jpg` and apply some decorative gradient shading,
> add a text that says Garden Walk and save as `garden-processed.jpg` in the
> same directory

| Before | After |
|---|---|
| ![Before](example-garden-before.jpg) | ![After](example-garden-after.jpg) |

### Look before deciding

The source is 3052x4060: bright sky and dense foliage across the upper two
thirds, a stone path along the bottom. That matters, because the path is the
only calm region — everything above it is too busy for type to survive on.
Which is what decides where the text goes and, in turn, what the shading is
for.

### Ask for the numbers rather than guessing

```
open ~/assets/gardenpath.jpg
info
measure text "Garden Walk" family="URW Bookman" size=290
```

```
gardenpath.jpg: 3052x4060, 1 layers
  document: 3052x4060, 1 layers
  measure "Garden Walk": 1909x348 (offset 0, -210)
```

That is the whole trick. 1909 wide on a 3052 canvas centres at
`(3052 − 1909) / 2 = 571`. The offset says the baseline sits 210 below the
ink's top, so a baseline of 3520 puts the type's top at 3310 — low on the
path, with room beneath it. No rendering, no guessing, no iterating toward a
number that was knowable up front.

### The script

```
open ~/assets/gardenpath.jpg

# A deep green-black wash rising from the bottom. It grounds the composition
# and gives the title something to sit on, instead of letting it fight the
# stone texture. Green-black rather than neutral, to stay in the picture's
# own palette.
layer new
set name="Shade — bottom"
gradient 0 4060 0 2450 from=#0a1a0cd8 to=#0a1a0c00

# A far subtler one from the top, so the bright sky does not run away with
# the eye and the frame feels closed.
layer new
set name="Shade — top"
gradient 0 0 0 1150 from=#08140a70 to=#08140a00

text 571 3520 "Garden Walk" family="URW Bookman" size=290 color=#f5ecd8
effect drop-shadow distance=16 size=38 opacity=0.55 angle=115

export ~/assets/garden-processed.jpg
```

Every size is scaled for a 4060-pixel-tall image. A shadow of `size=8` — a
sensible number on a screenshot — would be invisible here.

### Read the report back

```
gardenpath.jpg: 3052x4060, 4 layers
  [0] Background               Raster  at (0, 0) 3052x4060
  [1] Shade — bottom           Raster  at (0, 0) 3052x4060
  [2] Shade — top              Raster  at (0, 0) 3052x4060
  [3] Garden Walk              Type    at (397, 3136) 2257x696  fx: Drop Shadow
10 steps ran, 0 failed
```

The type layer reports 2257x696 at (397, 3136) — larger than the 1909x348 that
was measured, because the drop shadow reaches beyond the letters. Both numbers
are useful and they are not the same number: `measure` gives the ink, the
report gives everything the layer draws. A caller checking whether a title
fits inside the frame wants the second.

### Then look at it

The last step is one no report can do. Render, open the image, and check the
things a summary cannot describe: at 1:1, not downscaled, because a preview
hides exactly what goes wrong — antialiasing on the serifs, and banding across
a smooth wash. Both were clean here; if the gradient had banded, the fix would
have been the `dither` option that is on by default for precisely that reason.

### What the job exposed

Putting the pathway to real work immediately found two gaps that building it
had not: there was no `gradient` command at all, so decorative shading could
only have been faked with a shape at zero fill opacity; and a leading `~` was
treated as a folder of that name, so the very first line failed with a
complaint about a directory nobody had asked for. Both are fixed. That is the
argument for using a tool on something real before believing it is finished.

## What it does not do

No loops, variables or arithmetic — a caller that needs those has a real
language and can emit the script. Curves and levels take no parameters here
yet, and there is no way to paint a brush stroke: the pathway is aimed at
composition, type and effects rather than freehand work.
