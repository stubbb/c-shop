# What could come next

A list of what this editor does not have yet, why each would matter *here*
rather than in general, and roughly what it would cost. Compiled by working
through what the established editors offer and asking, of each, whether it
earns its place in a program with this one's shape: a single binary, a GPU
compositor, and a script surface that something without eyes can drive.

Nothing here is a commitment. It is a menu, ordered by what it would unlock.

## The three worth doing first

**Guides, rulers, grid and snapping.** ~~The most conspicuous absence.~~ Done.
Rulers on two edges, guides dragged out of them, a grid, and snapping that
catches by whichever edge comes closest. Guides are saved with the document.

**Anything at all that persists between runs.** ~~Nothing is remembered.~~
Done. Window size, tool, brush, colours, panels, view settings and the last
dozen opened files, as JSON under the usual configuration directory.

**Sixteen bits in the layers.** ~~The machinery is already there and stops at
the layer.~~ Done. A raster layer holds eight bits or sixteen, a sixteen-bit
file opens, saves and exports without losing a count, and `Image ▸ Mode` moves
between the two. The tools still paint in eight and say so; compositing is
capped at half-float, which wgpu's refusal of `Rgba16Unorm` as a colour
attachment makes a floor rather than a choice.

## Tools people reach for and do not find

**Healing brush, and its spot form.** ~~The clone stamp copies pixels; a
healing brush copies *texture* and keeps the destination's colour and
brightness.~~ Done, on `J`. The correction is fitted to the ring just outside
each dab rather than blurred out of the middle of it — the obvious way round
reproduces a fraction of the blemish, measurably no better than cloning.

**Dodge, burn and sponge.** ~~Lightening and darkening by hand, restricted to
shadows, midtones or highlights.~~ Done. On `O`, sharing the stroke engine, with
the range as a falloff rather than a band and Alt reversing the direction.

**Blur, sharpen and smudge.** ~~The filters exist; what is missing is applying
them through a brush.~~ Done, on `R`. Blur and sharpen are the ordinary stroke
reading a frozen copy of the layer; smudge writes as it goes, because what it
lays down depends on what it picked up.

**A history brush.** ~~Painting a region back to how it was at an earlier
point.~~ Done, on `Y`. Mark a state in the History panel and the brush paints
back to it: the walk to that state and back happens once, when it is marked,
rather than on every stroke.

## Being able to change your mind later

**Smart objects.** ~~A layer that remembers where its pixels came from and can
re-render them.~~ Done. `Layer ▸ Convert to Smart Object`; after that a
transform composes onto the placement and the picture is re-rendered from the
source, so the twentieth is as good as the first and a placement costs the
history nothing. Saved with the source and the placement rather than the
rendering, which can be worked out.

What is not there yet is the *linked* half: one source shared between several
layers, so changing it updates every place it was used. That needs a
document-level store with references rather than a layer that owns its own
picture, which is a bigger change than the rest of this was.

**Filters as layer attachments.** ~~Adjustments can already be non-destructive
layers; filters cannot.~~ Done. A stack of them per layer, each with its own
switch and opacity, a shared mask, and a Smart Filters panel beside the layer
they are on. The layer's own pixels are never touched, so changing a radius is
changing a number.

**Vector masks.** ~~Paths exist, shapes exist, masks exist; a path *as* a mask
does not.~~ Done. A mask can keep the path it was drawn from, so resizing the
document draws it again at the new size rather than resampling the last drawing
of it — which is measurably the difference between an edge that stays crisp and
one that spreads.

**Layer states.** ~~Remembering a set of visibilities, positions and styles by
name.~~ Done. A state records what each layer is *doing*, not what it contains,
so an edit made after a state was saved is still there when the state comes
back — which is the whole reason to keep two versions in one document rather
than in two.

## Selecting and masking

**Selection by colour range.** ~~Across the whole image rather than
flood-filled from a point.~~ Done, on `Select ▸ Colour Range…` and
`select colour`. Sampled colours, tonal bands or a hue band, with fuzziness
giving *partial* coverage — the real difference from the wand, whose answer is
in or out.

**Refining an edge.** ~~The model-driven segmentation gets the shape right and
the boundary approximate.~~ Done, on `Select ▸ Refine Edge…` and
`select refine`. A guided filter fits the mask to the picture's own brightness
locally, so the boundary moves onto the edge that is really there instead of
being moved by hand. The radius has to reach the edge to find it, which the
window says.

**Paths and selections, both ways.** ~~A selection from a path, and a path
traced round a selection.~~ Done. The trace reuses the crack-following the
marching ants already do; what is new is Douglas–Peucker on top of it, which is
what turns a staircase of a few hundred right angles into a path with handles
someone can actually edit.

## Geometry

**Warp and puppet warp.** ~~Free transform handles the corners; nothing bends
the middle.~~ Done, both on `Edit ▸ Transform`. One engine — moving least
squares — with two ways of collecting its input: a mesh over the layer, or pins
put where they are wanted. The rigid fit is what keeps an arm looking like an
arm when it is moved, since an affine one will happily squash it to reach.

**Content-aware scale.** ~~Changing an image's proportions while leaving the
things in it alone.~~ Done, as a checkbox in Image Size and `resize
content-aware`. The selection protects what it covers, which is where the
segmentation work pays off. It runs on a worker thread with a progress bar,
because it takes seconds on a large photograph — down from three quarters of a
minute, once the energy stopped being rebuilt for every seam.

**Perspective crop.** ~~Straightening a photographed rectangle in one gesture.~~
Done, as a checkbox on the Crop tool and `straighten`. Put the four corners on
something rectangular and cropping undoes the projection that made it a
quadrilateral.

**Aligning several frames.** ~~Stitching a panorama, or stacking frames for
noise or focus.~~ Done, on `Layer ▸ Align Layers` and `align`. Harris corners,
oriented binary descriptions, matching with the ratio test, and RANSAC over a
shift, a similarity or a full projective fit. Two photographs of different
things are refused with a reason rather than aligned to whatever the arithmetic
produced — a least-squares fit through matches that agree on nothing collapses
to "send everything to one point", and that had to be caught explicitly.
`Align and Stack` averages the result, which is how noise comes off a sequence.

What is not here is blending a panorama: the frames are aligned and left as
layers, so a seam between two exposures is still a seam.

## Files it cannot open

**Raw camera files.** Still the most serious omission: a photographic pipeline
whose front door is missing. A camera database is the part that cannot be
written, but it is also the part DNG does not need — a DNG carries its own
colour matrix, black and white levels, white balance and CFA pattern, so a
reader for *self-describing* raw is a real feature without a database behind
it. That is the shape this should take when it is taken, and it is still the
largest piece of work on this page.

**SVG and PDF.** ~~Vector in, vector out.~~ Done, in the directions each is
worth doing. An SVG opens as shape layers and saves back as paths, so a round
trip returns editable geometry rather than a picture of it — paths, rects,
circles, ellipses, lines, polylines and polygons, with transforms composing
through nesting and arcs converted to cubics. What it cannot draw — text,
gradients, patterns, filters, clipping — it names, rather than dropping
silently and leaving a picture that is wrong in a way nobody can see.

PDF goes out only. Writing a page around an image is a few hundred bytes of
structure; reading one is object streams, a dozen filters, embedded fonts and a
general page-description language, which is a project rather than a feature —
and reading the easy tenth of it would open some files and quietly mangle
others.

**HEIC and AVIF.** Still not here, and this is why: both wrap a *video* codec —
HEVC and AV1 — and a decoder for either is tens of thousands of lines that
nobody should write twice. The libraries exist; this build has no network and
no vendored copy of them, so the honest position is that these two wait on a
dependency rather than on effort.

**Frames and a timeline.** ~~For animated GIF and APNG.~~ Done. Opening an
animation used to give back its first frame and discard the rest silently,
which is the worst way to not support something: the file opens, looks right,
and is not what was in it. Now every frame becomes a layer with a timeline over
them, so painting, masks, adjustments and effects all work on a frame without
being taught anything — a frame *is* a layer, and showing one is a matter of
visibility. Frames are composed on the way in, so what you get is what each
moment looked like rather than the rectangle that changed. Both formats write
back out, whole.

## Building on the models that are there

Each of these is mostly composition of pieces that already exist.

**Replacing a sky.** The labeller already finds the sky, the inpainter already
fills what is removed, and the colour work already knows how to match one
picture's light to another's. What is missing is the single command that does
all three and the judgement about the horizon.

**Retouching a face.** A face detector, then the denoiser at low strength
inside the mask — smoothing skin without smoothing eyes and hair.

**Colouring a photograph that has no colour.** There is no usable exported
model for this that I could find; it would need one to be converted first.

**Effects that use the depth.** Fog that thickens with distance, a shallow
depth of field applied after the fact, a parallax nudge. The depth is already
computed and already available as a mask; these are ordinary filters that read
it.

## Under the floor

**A colour-managed canvas.** Documents carry a profile and the canvas shows
their numbers directly, which is right for sRGB and a lie for anything else.
This also brings soft proofing — seeing a picture as a press would print it,
which the CMYK work set up and did not finish.

**Pressure from a tablet.** The stroke engine has opacity, flow and hardness
and no way for a pen to drive any of them.

**Custom brushes and patterns**, defined from a selection rather than chosen
from the built-in set.

**Choosing your own shortcuts**, and seeing them written down somewhere other
than the reference document.
