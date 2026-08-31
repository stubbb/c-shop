# What could come next

A list of what this editor did not have yet, why each would matter *here*
rather than in general, and roughly what it would cost. Compiled by working
through what the established editors offer and asking, of each, whether it
earns its place in a program with this one's shape: a single binary, a GPU
compositor, and a script surface that something without eyes can drive.

Nothing here was a commitment. It was a menu, ordered by what it would unlock,
and it has now been worked through end to end — every entry struck out below is
built, tested and documented. Five things are deliberately still open, and each
says so where it sits: painting at sixteen bits, blending an aligned panorama,
loading custom patterns, compositing a smart-filtered layer off the drawing
thread, and colourising a photograph that has no colour. The last waits on a
model that can be fetched; the rest wait on work.

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
between the two. Compositing is capped at half-float, which wgpu's refusal of
`Rgba16Unorm` as a colour attachment makes a floor rather than a choice.

**Still open: the tools paint in eight bits.** They say so — the window offers
`Image ▸ Mode` rather than narrowing a layer behind your back — but saying so
is the honest half of the situation and not the whole of it. The stroke's
coverage mask is already independent of depth, so what is left is the
compositing at the end of a stroke and the sampled sources reading and writing
at the layer's own depth. Not deep, but every path in `paint.rs`, and half of
it done is worse than none: a brush painting at sixteen bits into a stroke
buffer that clamps at eight would look right and quantise anyway.

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

**Raw camera files.** ~~The front door of the photographic pipeline.~~ Done,
for the raw files that describe themselves. A DNG carries its own colour
matrix, black and white levels, white balance and filter pattern, so it needs
no database of camera models — and that database is the part of raw support
that cannot be written, only accumulated, one body at a time, forever. So DNG
is read and the proprietary formats are refused with that explanation rather
than opened and guessed at.

A TIFF reader for the tags, a lossless-JPEG decoder for the data — process 14
from the same standard as ordinary JPEG and almost nothing else in common: no
transform, no quantisation, no loss — and then black subtraction, white
balance, demosaicing against the green channel, and the camera's own matrix
into sRGB. Sixteen bits out, since narrowing there would throw away the reason
for shooting raw.

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

**Replacing a sky.** ~~The pieces exist; the command that joins them does
not.~~ Done, on `Image ▸ Replace Sky` and `sky`. The judgement about the
horizon turned out to be three judgements: where it is (the lowest row that is
*mostly* sky, not the lowest row containing any — a gap between two buildings
is not a horizon), which way to soften the join (into the sky, since sky on a
branch reads as sky seen through it and a branch in the sky reads as a
mistake), and how far to grow the mask first (a label boundary falls just
inside the sky, and what is left behind is the pale fringe that announces a
replacement from across a room).

**Retouching a face.** ~~A detector, then smoothing inside the mask.~~ Done, on
`Image ▸ Retouch Skin` and `retouch`. The surface blur turned out to be exactly
the right tool and was already here: its threshold is the distinction between
texture and feature, which is the whole problem. Some of the texture goes back
afterwards, because skin with no grain at all is what "retouched" looks like
from across a room. The detector finds people rather than faces, so the head is
taken as a share of the box — a share that depends on how much of the body is
in frame.

**Colouring a photograph that has no colour.** Still not here, and still for
the same reason: there is no usable exported model, and this build has no
network to fetch one with even if there were. It waits on a conversion, not on
effort.

**Effects that use the depth.** ~~Fog, a shallow depth of field, a parallax
nudge.~~ Done, on `Image ▸ Depth Effects…` and `haze`, `focus`, `parallax` —
three effects sharing one depth map, because working the depth out is the
expensive part.

Parallax had to be turned inside out. Asked forwards — where does each pixel
go — rounding sends two neighbours to the same place and skips the one between,
so a solid object comes out with a comb of one-pixel holes through it. On a
photograph that showed as streaks along every depth cliff, which is how it was
found. Asked backwards — which pixel ends up here — every pixel is filled
exactly once by construction, and where two land in the same place the nearer
one wins, which is what occlusion is.

## Under the floor

**A colour-managed canvas.** ~~The canvas shows a document's numbers directly,
which is right for sRGB and a lie for anything else.~~ Done, on
`View ▸ Screen Profile` and `View ▸ Proof Colours`. The colour engine runs on
the processor and the canvas is drawn on the card, and neither can do the
other's job — so a three-dimensional table is built once when the profiles
change and read once per pixel per frame. A document in the screen's own space
gets the identity table and is shown exactly as before, which is a test.

Soft proofing came with it: document → press → screen, so what the press cannot
reach comes back as the nearest thing it can. Proofing through four inks needs
the ink path rather than the ordinary conversion, which refuses a destination
that is not three channels — swallowing that error showed the picture unproofed
and let someone trust it.

**Pressure from a tablet.** ~~The stroke engine has opacity, flow and hardness
and no way for a pen to drive any of them.~~ Done. Which of them pressure
drives is a choice rather than a default — size alone is the pencil, flow alone
the airbrush — and a device that cannot measure pressure presses fully, so a
brush behaves exactly as it always did unless something is actually reporting.
Pressure interpolates along each segment, because a pointer sends a handful of
samples a second and a stroke lays down dabs far faster; stepping at each
sample makes a visibly banded line.

**Custom brushes**, ~~defined from a selection rather than chosen from the
built-in set.~~ Done. The selection's coverage becomes the tip, normalised so
something faint still paints at full strength, and its longer side is fitted to
the brush size with its shape kept — stretching it to fill the dab's square
instead makes a wide, thin tip stamp as a square, which is the one thing a
shaped brush must not do.

Custom *patterns* are still generated rather than loaded — a tile taken from a
selection needs somewhere to keep it and a way to pick it, which is a library
rather than a brush.

**Choosing your own shortcuts.** ~~And seeing them written down somewhere other
than the reference document.~~ Done, on `Edit ▸ Keyboard Shortcuts…`. Only the
changed ones are stored, so a later build's new defaults reach everyone who has
not overridden them. Taking a chord takes it from whoever had it — two commands
on one chord means one of them silently never runs — and the displaced command
is recorded as having none rather than left at its default, which the next run
would give back.

**Long work off the drawing thread.** ~~A filter, a resize, an alignment and a
content-aware scale all held the frame while they ran; on twelve megapixels
that is between a third of a second and eight seconds of a window that stops
repainting and gets offered to the user for killing.~~ Done. There is one
mechanism — a named job on a thread, a counter it writes and the status bar
reads, and a flag it checks to know it has been told to stop — and everything
slow goes through it, including the eight model runs that were each doing their
own version of it with a spinner and no way to stop.

Two things came out of the same pass. `resample::transform` and
`resample::resize` were written a row at a time in parallel, which is a sixfold
speedup and bit-identical output — a rotation on twelve megapixels went from
1.2 seconds to 0.18. And a filter now checks that the pixels it read are still
the pixels that are there before writing its answer back, because otherwise a
four-second filter silently undoes a brush stroke made while it ran.

**Still open: compositing a smart-filtered layer.** The compositor runs a
layer's filter stack on the drawing thread whenever the layer is dirty — 144 ms
for one modest blur on twelve megapixels, 308 ms for two filters — so painting
on a filtered layer stutters. Moving it to a worker means deciding what the
canvas shows while the new version is worked out, and showing someone stale
pixels is a decision about what the program *is*, not a refactor.
