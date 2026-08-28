# Architecture

The parts of C-Shop that are not obvious from reading the code, and the reasons
behind them. Everything here was a decision with a live alternative, and most
of them were forced by something that went wrong first.

## Colour: blending happens in gamma space

Every colour in the compositor is **sRGB-encoded**, not linear light. Layer
textures hold encoded bytes, the working buffer holds encoded floats, and blend
modes operate on those encoded values directly.

That is physically wrong and deliberately so. Blending in linear light is what
light actually does, but it is not what established image editors produce, and
a 50% grey brush on white has to land where users expect it to land rather than
where a photon would. Matching the result people already know matters more here
than matching physics. `cshop-core/src/color.rs` carries the full rationale.

The one place linear light is correct — and used — is resampling and filtering,
where premultiplied linear values avoid dark fringes around transparent edges.

### The gamma bug this caused

Because the working buffer is gamma-encoded, everything handed onward has to
agree about that. For a while it did not: the canvas texture given to egui was
in an `*Srgb` format, so the hardware linearised it on sample, and egui's own
shader — which states "we expect normal textures that are NOT sRGB-aware" —
linearised it again. Every pixel on screen was `srgb_to_linear(value)`: mid grey
128 displayed as 55, while the file saved correctly.

The lesson is in `cshop-gpu/src/texture.rs`: `DISPLAY_FORMAT` is deliberately
**not** an `*Srgb` format, and a test asserts it, because reading the texture
back cannot tell the two cases apart — the stored bytes are identical either
way. Only the format distinguishes them.

## The compositor cannot use fixed-function blending

The obvious design is one draw call per layer with the GPU's blend state set
from the layer's blend mode. It does not work. Fixed-function blending computes
`f(src, dst)` from a fixed set of factors, and modes like Multiply, Overlay and
Luminosity need the *backdrop* as a shader input — with correct alpha
compositing on top, which the blend equation cannot express.

So each layer is a full-region pass that **samples** the backdrop as a texture.
Two scratch textures alternate: read from one, write to the other, swap. Scratch
is confined to the dirty rectangle and tiled at 2048 pixels, so memory stays
constant no matter how large the document is.

A 24-megapixel document with ten layers composites in about 12 ms.

## Alpha conventions

Straight (non-premultiplied) alpha at rest in the document model, because that
is what image files hold and what a user editing a pixel means. Premultiplied
inside the compositor, and in every resampling and filtering operation, because
interpolating straight alpha pulls the colour of fully transparent pixels into
the result and produces dark fringes.

Both conventions are correct in their own place; the bugs come from crossing
between them silently, so every conversion is explicit.

## Vector layers carry their own raster

Type and shape layers are re-editable: they store their description — the
string and its style, or the geometry and its fill and stroke — and re-render
whenever it changes. But they also cache the resulting pixels, and
`Layer::pixels()` hands that cache to everything downstream.

The payoff is that the compositor, masks, blend modes, opacity, filters,
thumbnails, merge, flatten and save need **no knowledge of vectors at all**. A
shape layer is a raster layer as far as they are concerned. The only code that
knows the difference is the code that edits it, plus `pixels_mut()`, which
returns `None` so that painting on a live vector layer is refused rather than
being silently discarded on the next re-render.

Each cached raster records where its **anchor** — the type's insertion point,
the shape's box corner — falls inside it. That keeps `Layer::offset` the single
source of position: widening a stroke or typing more text grows the raster
without the layer appearing to move, and moving a layer needs no knowledge of
what kind it is.

## Layer effects are all one distance field

Every effect — shadow, glow, bevel, stroke, satin — is some function of *how
far a pixel is from the layer's edge*. A stroke is a band around distance zero;
a glow is a ramp away from it; spread and choke move the contour before
blurring; a bevel lights a height map built from it. So the renderer computes
one signed distance field per layer and every effect reads it. Offsetting a
shadow is then sampling that field at a shifted position rather than rebuilding
anything.

Two things about that field were worth getting right. It has to be
**continuous**: the distance transform is exact but measures to the nearest
pixel *centre* of a binary mask, so it reads about half a pixel long and knows
nothing about an antialiased edge. Correcting only near the edge introduces a
step, and the bevel differentiates the field — a step becomes stripes down
every diagonal. And the gradient is taken with a **Sobel** stencil rather than
central differences, because a height map built from a distance field has a
crease along the shape's medial axis; on a diagonal that crease alternates with
pixel parity, and central differences turn it into a plaid.

The two overlays are the exception to the distance field: a gradient overlay
reuses the Gradient tool's own geometry so the shapes match exactly, and a
pattern overlay evaluates a repeating figure directly. Both are clipped to the
layer's alpha and neither reaches outside it, so they add nothing to the
layer's drawn extent. The gradient's ramp is baked into a 256-entry table
first, because `color_at` sorts its stops on every call — the same mistake that
made the Curves preview freeze.

Effects are composited on the CPU into a raster that is handed to the GPU like
any other layer, with the layer's own pixels already scaled by fill opacity and
the effects deliberately not. That is what makes a stroke-only layer work, and
it means the compositor needs no knowledge of effects — only that the layer
draws over a larger rect than its own pixels, which `Layer::render_bounds`
reports.

## Shapes are distance fields, not scanlines

Every shape has a cheap signed distance function, and one distance gives both
the fill and the stroke: the fill is where the distance is negative, the stroke
is a band around zero. Antialiasing becomes a clamp rather than a coverage
integral, fill and stroke stay perfectly registered, and inside, centred and
outside strokes differ only in where the band is centred — one line, three
behaviours.

## Adjustments are prepared once

The table-driven adjustments — Levels, Curves, Brightness/Contrast, Exposure,
Invert, Posterize, Threshold — bake a 256-entry lookup table. `Adjustment::apply`
builds that table before reading a single entry from it, which is correct for
one colour and catastrophic in a loop: dragging a point in the Curves dialog
was rebuilding the table *per pixel*, at 256 spline evaluations each.

`Adjustment::prepare` bakes once. The preview went from 611 ms to 1.5 ms, and a
full-resolution apply from 11.4 s to 0.6 ms, byte-identical. The lesson is
encoded as a test that runs both paths and compares them.

## Previews cost a fixed amount

Filters are far too slow to run on a full-resolution layer while a slider moves
— a 24-megapixel radial blur takes two seconds. Every preview is therefore
bounded rather than proportional:

- **Adjustments** preview on a 320-pixel proxy. They are pointwise, so a proxy
  means the same thing at any resolution.
- **Filters** never process more than a viewport's worth of pixels, whatever
  the document size. A filter with bounded support — the blurs, sharpen,
  median, high pass — is previewed from a **crop of the full-resolution
  source**, taken with a margin so pixels at the crop's edge still see real
  neighbours. Zoomed to 100% that is not an approximation: it is exactly what
  the applied filter will produce, and there is a test comparing the two.
  Filters that depend on the whole image — the distortions, Average, the cell
  filters — cannot be cropped without changing their result, so those render
  whole at fit scale and the label says so rather than implying detail that is
  not there.

## Undo is one step per gesture, not per event

Typing a word, dragging a slider and painting a stroke are each a single
history entry. Live editing writes to the document directly, outside the undo
stack, and the whole session is recorded when it is committed. A `Compound`
command groups operations that a user performed as one action — Layer via Copy
both adds a layer and clears the selection, and one undo has to take back both.

A stroke that changed nothing never becomes a history entry, which is what
tells you the clone stamp's source has wandered off the canvas instead of
leaving an "undo" that undoes nothing.

## The project format is written by hand

A derive-based encoding ties the file layout to the *order* of fields and enum
variants, so reordering a struct silently changes the format and corrupts every
file already written. Documents outlive the code that wrote them, so `.cshop`
is spelled out byte by byte in `cshop-io/src/project.rs`, where changing the
layout is a deliberate act rather than a side effect of tidying a struct.

The file is a short header followed by tagged chunks, so a reader skips what it
does not recognise: a newer file loses only the parts an older build has no
idea about instead of failing outright. Pixel and mask data are deflated;
everything else is small enough not to matter.

PSD is a different problem — the layout is someone else's and fixed. The part
worth knowing is that it has no nesting: layers are a flat list stored bottom
to top, and a group is a pair of marker entries around its children, a bounding
divider below and a header carrying the name above. Reading walks bottom to top
opening a scope at one and closing it at the other; writing emits the same
pair. Channels are PackBits run-length encoded, one plane per channel, with the
per-row byte counts ahead of the data.

## Testing what usually goes untested

Three classes of bug kept getting through ordinary unit tests, so each has its
own harness:

**The GPU disagreeing with the CPU.** Blend modes and adjustments are
implemented twice and compared pixel by pixel. This catches shader bugs that no
amount of reading the WGSL would.

**Widgets covering other widgets.** egui draws the widget registered *last* on
top. Registering a window-drag handle last made it cover its own menus and the
close button, and the whole title bar went dead. Every prior test called app
methods directly and never exercised hit-testing. `input_harness.rs` now drives
the real `CShopApp::update` with synthetic pointer and keyboard events, so
these fail in CI instead.

**Anything visual.** The `--screenshot` path runs the identical egui and
compositor pipeline offscreen, so the interface can be rendered and inspected
without a display server.

## Constraints worth knowing

- **No system packages.** X11 is opened via `dlopen` (`x11-dl`), Vulkan via
  `ash`, and every other dependency is pure Rust. This was a hard requirement
  and it shaped the dependency list.
- **Fonts come from the system**, scanned on a background thread at startup
  (182 families in about 120 ms on a typical Linux install) rather than
  bundled: an editor should offer what the user has installed, and 80 MB of
  font data does not belong in a binary.
- **`Rgba16Float` is the working format** because `Rgba16Unorm` — which would
  suit gamma-encoded values better — cannot be used as a colour attachment in
  wgpu.
