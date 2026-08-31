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

**Selection by colour range**, across the whole image rather than flood-filled
from a point, which is what the magic wand does.

**Refining an edge.** The model-driven segmentation gets the shape right and
the boundary approximate; hair and fur need a matting pass that estimates
partial coverage rather than a hard in-or-out.

**Paths and selections, both ways.** A selection from a path, and a path traced
round a selection.

## Geometry

**Warp and puppet warp.** Free transform handles the corners; nothing bends the
middle.

**Content-aware scale.** Changing an image's proportions while leaving the
things in it alone — seam carving, which pairs naturally with the work already
done on finding what is in a picture.

**Perspective crop.** Straightening a photographed rectangle in one gesture
rather than by lens correction and then a crop.

**Aligning several frames.** Stitching a panorama, or stacking frames for
noise or focus. The feature detection this needs is a large piece of work, and
the payoff is two features that nothing else here approaches.

## Files it cannot open

**Raw camera files.** The most serious omission for a program that has just
gained colour management, sixteen-bit output and CMYK separation — that is a
photographic pipeline whose front door is missing. Also the largest single
piece of work on this page, since it means demosaicing and a camera database.

**SVG and PDF.** Vector in, vector out, on a program that already has paths and
Bézier geometry.

**HEIC and AVIF**, which is what a phone now produces.

**Frames and a timeline**, for animated GIF and APNG — the format is already
read.

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
