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

**Anything at all that persists between runs.** Nothing is remembered: not the
window size, not which tool was selected, not the last brush, not recently
opened files, not the workspace. Every launch starts from the same blank state.
A small settings file under the usual directory would fix all of it at once,
and it is the sort of absence people notice on the second day rather than the
first.

**Sixteen bits in the layers.** The machinery is already there — `Rgba16`, a
pixel buffer generic over its sample, files and profiles that read and write at
either depth — and stops at the layer. Until it goes further, `depth=16` on
export preserves what the compositor worked out rather than what was painted,
which the documentation says plainly and which is still a gap. This is the
largest of the three by some way, and the one with the most already built.

## Tools people reach for and do not find

**Healing brush, and its spot form.** The clone stamp copies pixels; a healing
brush copies *texture* and keeps the destination's colour and brightness, which
is why it works on skin and a clone stamp does not. The inpainting model covers
"remove this whole object" and leaves the small, precise repair uncovered.

**Dodge, burn and sponge.** Lightening and darkening by hand, restricted to
shadows, midtones or highlights. Old tools, still the fastest way to shape a
photograph, and a natural fit for the existing stroke engine.

**Blur, sharpen and smudge.** The filters exist; what is missing is applying
them through a brush rather than to a whole layer or selection.

**A history brush.** Painting a region back to how it was at an earlier point.
The history already holds the states; this is a way of reaching them locally
instead of globally.

## Being able to change your mind later

**Smart objects.** A layer that remembers where its pixels came from and can
re-render them — so a photograph placed and scaled down can be scaled back up
without having lost anything, and a change to the source updates every place it
was used. This is the single biggest structural difference between this editor
and the ones it is measured against.

**Filters as layer attachments.** Adjustments can already be non-destructive
layers; filters cannot. A blur that stays editable, on a layer, with its own
mask, is the same idea applied to the other half of the program.

**Vector masks.** Paths exist, shapes exist, masks exist; a path *as* a mask
does not. Mostly a matter of wiring three things that are all already built.

**Layer states.** Remembering a set of visibilities, positions and styles by
name, so two versions of a design can live in one document.

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
