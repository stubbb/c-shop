# The vision pack

Two neural networks, as something you opt into.

```sh
vision/setup.sh
```

That is the whole installation: a Python environment and about 610 MB of model
weights, under `~/.cache/cshop/vision`, which comes to a little under a
gigabyte with the runtime. Nothing in the editor needs it: open, edit
and save behave exactly as before whether it is there or not, and only eight
commands — `detect`, `segment`, `denoise`, `upscale`, `separate`, `inpaint`,
`depth` and `relight` — and five windows ask for it.

## Why it is a separate process

C-Shop is one binary with almost no dependencies that builds offline. A neural
network runtime is tens of megabytes of platform-specific machine code that
changes every few months. Putting the second inside the first would cost the
first its whole character, for a feature most sessions never touch.

So the boundary is a process and a line of JSON. `vision/cshop-vision.py` reads
an image and a prompt and prints an answer; the editor runs it and reads that.
Not installing it costs nothing and breaks nothing.

## What each model is for

**YOLOv8n** says what is in a picture and where — a class, a confidence and a
box. It knows [eighty kinds of thing][coco] and nothing else. That limit is
worth knowing before you are surprised by it: it finds a dog, a person, a
phone, a chair, a boat. It does not find a sky, a mountain, a road, a building
or a plant, because those are not on the list. Asking it for one is not a
failure of the picture.

**MobileSAM** does the opposite. It has no idea what anything *is*; given a
point or a box it separates whatever is there from its surroundings. So it
works on a mountain or a wall as readily as on a dog — but it needs telling
where to look.

Which is why they are better together than either alone: YOLO finds the dog
and says where, SAM cuts it out.

**Depth Anything** guesses how far away everything is. Not a model of the
scene — one number a pixel, with no idea what is behind anything — but enough
to work out which way a surface *faces*, which is what light responds to.

**LaMa** fills a hole in with what was probably behind it. Given a picture and
a mask it invents the covered part from what surrounds it, which is how an
object is *removed* rather than merely painted over.

**SegFormer** labels every pixel with what it is — a hundred and fifty kinds of
thing from [ADE20K][ade], and pointedly the kinds YOLO has never heard of: sky,
mountain, road, building, water, plant, earth. Where YOLO answers "there is a
dog, here", SegFormer answers "this pixel is sky and that one is a tree". It
does not cut anything out; that is still SAM's job.

[ade]: https://groups.csail.mit.edu/vision/datasets/ADE20K/

**Real-ESRGAN** enlarges, four times up, inventing the detail a bigger sensor
would have recorded. Five megabytes for the compact "general" variant, which is
a hundredth of what the denoiser weighs and about as quick.

**SCUNet** removes noise. A Swin-Conv-UNet: Swin transformer blocks inside a
UNet, which is what makes it quick enough to be worth waiting for — the UNet's
downsampling is why it costs a fifth of what a transformer working at full
resolution does. The variant here is the one trained on real photographic
degradation rather than on synthetic noise of a known strength, which on this
machine's measurements beat the fixed-sigma models at every noise level tried
*and* was far gentler on a picture that was not noisy in the first place. That
last part is what matters most, because most of any photograph is not noisy.

[coco]: https://cocodataset.org

## From a script

```
open photo.jpg
detect                          # what is in here?
segment class=dog feather=1     # cut that out, softening the edge a little
layer via-copy                  # lift the selection onto its own layer
layer select 0
layer delete                    # and drop the background
export dog.png                  # PNG keeps the transparency
```

`detect` reports each object into the run's facts, so a caller reading the JSON
report gets the classes, confidences and boxes without parsing prose. `segment`
leaves a **selection**, not a new layer, so everything the editor already does
with one applies — feather it, invert it, fill it, duplicate through it — with
no second vocabulary to learn.

| | |
|---|---|
| `detect [class=] [conf=]` | Find objects. `class=dog` keeps only those; `conf=` is the threshold, 0.25 by default. |
| `segment class=dog` | Detect that class and cut out the best match. |
| `segment box=x0,y0,x1,y1` | Cut out what is in that rectangle. |
| `segment point=x,y` | Cut out what is at that point. Several as `x,y;x,y`. |
| `segment point=… not-point=x,y` | And exclude what is at these. |
| `segment … expand=3` | Grow the result outward, up to 50 pixels. |
| `segment … feather=2` | Soften the edge of the result, in pixels. |

With nothing said, `segment` uses whatever `detect` last found — which is the
whole point of running them in sequence.

## From the window

**Select ▸ Segment Object…** opens a window that is not modal, because the
canvas is its control as well as its preview. Click the thing you want; the
selection appears. Click again to refine, Alt-click to say "not this", and use
the two sliders on the edge: *Expand* grows the selection outward, for when the
model has cut a little inside the subject, and *Feather* softens it. Both work
on the mask the model has already returned, so neither costs a second look at
the picture. *Find objects* lists what the detector recognises, if anything, so
a dog can be chosen by name instead of by aim.

Cancel puts the selection back as it was.

## Removing noise

```
open photo.jpg
select 40 30 200 160     # only this part, which is much quicker
denoise                  # → removed noise over 200x160 at 40,30: 2 tiles,
                         #    moved 16.7 levels a channel
```

Or **Filter ▸ Remove Noise…**, which does the same by hand and tells you how
long it is likely to take before you commit to it.

### What it costs

About **twenty-six thousand pixels a second**, on every core the machine has —
measured on sixteen of them, so a smaller machine will be slower. That is the
single most important thing to know about this tool:

| | |
|---|---|
| a 480×360 crop | 8 seconds |
| a 900×600 picture | 20 seconds |
| a 2 megapixel picture | a minute and a half |
| a 24 megapixel frame | a quarter of an hour |

So **select the part that needs it**. The noise that bothers anyone is usually
in a sky or a shadow, not across the whole frame, and cleaning a corner takes
seconds where cleaning everything takes minutes. Both the window and the script
work on the selection when there is one.

### The shape of the window

There is no live preview, because there is no viewport small enough to make one
feel live. What there is instead is the shape the cost actually has: the model
runs **once**, behind a progress bar, and then **strength** mixes its answer
back over the original — which is instant. So the judgement everybody actually
wants to make, *how much of this do I want*, is made after the waiting rather
than before it, against the real picture at full size. Keep commits it as one
history entry; Cancel puts the original back.

### How well it works

Measured, on a photograph with gaussian noise added at σ=22:

```
noisy     21.96 dB
denoised  34.72 dB      (+12.76)
```

And on a real one — a phone at night, twelve megapixels, at one pixel to one
pixel:

![Before and after, at full resolution](example-noise-detail.jpg)

Where there is no reference to measure against, high-frequency energy stands in
for it. Across that frame the sky lost 92% of what it had and the town below it
only 79%: the same pass, told apart by whether what it found was noise or a
window frame. The whole twelve megapixels took 7 minutes 40 seconds, against
the 8 minutes the window predicted from the rate above.

The picture is taken in overlapping tiles of 256 pixels, each weighted by a
taper that falls to nothing at its edge, so tiles cross-fade instead of butting
up against each other with slightly different ideas about the noise. That it
works is measurable rather than a matter of opinion: the average pixel-to-pixel
step *at* a tile boundary is 0.01080, and elsewhere in the same picture 0.01082.

Alpha is never touched. Noise is a property of the colour a sensor recorded;
coverage is not something a camera measured, and running it through a denoiser
would soften the edge of a cut-out for no reason.

### What it is not

It is one model with one idea of what noise looks like. It is very good on
sensor noise and grain, and it will also quietly remove fine texture that
happens to resemble them — skin pores, distant foliage, fabric weave. That is
what `strength` is for, and why the window lets you move it *after* seeing the
result rather than guessing beforehand.

## Lighting a photograph again

```
open dog.jpg
relight azimuth=200 elevation=18 intensity=1.5 ambient=0.45 relief=1.6 color=#ffd9a8
```

Or **Layer ▸ Relight…**, where the lamp is a dot on a circle rather than two
numbers: the middle is straight at the subject, the rim is level with it, and
the window says "from the top right" so nobody has to think in degrees.

![The photograph, the depth it read, and the same picture lit from the right](example-relight.jpg)

`depth` on its own puts the middle picture into the document as a layer, and
`depth mask` puts it straight on as a **layer mask**: near reveals, far hides,
so an adjustment clipped to it lands on the subject without anyone selecting
anything. `depth mask invert` is the other way round, which is what haze does.

**Layer ▸ Layer Mask ▸ From Depth** does the same, near or far, and the two
entries beside it — **Layer to Mask** and **Mask to Selection** — are what make
masks, layers and selections interchangeable rather than dead ends.

### Why the edges need softening

A depth model draws a *cliff* at the edge of an object: the dog is here and the
trees are four metres behind it, with nothing in between. Differentiating a
cliff gives an enormous slope, and an enormous slope lights as a hard black
line — so the first version of this traced a jagged outline round everything in
the picture.

Two things fix it, and both are on by default. The shape is **softened** before
it is differentiated, which turns the cliff into a slope the eye reads as a
turning surface; `softness` is that control, as a fraction of the shorter side.
And the slope is **capped**, because past about seventy degrees it is not a
surface at all — it is one object in front of another, and nothing in the
picture says how they join.

### From depth to shape

Depth alone lights nothing. What a lamp responds to is which way a surface
faces, and that is the *gradient* of the depth: where it changes quickly across
the frame the surface is turned away, where it is flat the surface faces the
camera. So the normal at a pixel is `(-dz/dx, -dz/dy, 1)`, scaled by **relief**
— which is the honest control, because the depth is relative and has no unit,
so how much shape a given change in it implies is a choice rather than a
measurement.

The depth is written at sixteen bits for exactly this reason. Lighting reads
the gradient, and at eight bits a gentle slope arrives as a staircase and
lights like one.

### What it is not

It is not physical relighting. Nothing here knows about shadows cast by one
object onto another, about how shiny anything is, or about what colour the
light already was — the new lamp is added to the picture rather than replacing
what lit it. On a portrait or an animal or a still life it is convincing; on a
scene whose lighting *is* the subject it will look like what it is.

The direction convention is pinned down by tests on a ramp rather than on a
photograph, because a photograph will look plausible lit from either side and a
slope will not.

### The shape of the window

The same as the others here, and for the same reason: the depth is the slow
half — a third of a second — and it does not change while the lamp moves, so it
is worked out once and everything after it is arithmetic on the picture. Moving
the lamp writes straight to the canvas without a history entry, so a session of
trying things leaves one step to undo rather than forty.

## Making something disappear

The three that find things and the one that fills holes compose into the thing
people actually want:

```
open dog.jpg
detect class=dog             # → found 1: dog 90% at 4,274 569x450
segment class=dog expand=20  # the dog becomes the selection,
                             #   with room round it
inpaint                      # → filled in 588x483 at 0,258
```

![The dog, and the bench without it](example-fill-in.jpg)

Seven seconds, three models, and nobody had to say where anything was. The
bench slats continue through where the dog sat and the foliage behind is
rebuilt.

The `expand=20` matters more than it looks. A mask that hugs an object leaves
that object's own edge inside the picture, and the model dutifully continues
it — so what survives is a faint outline of the thing that was removed. At 6 it
leaves fur on the bench; at 20 it does not; at 40 it starts eating the hand. **Edit ▸ Fill In Selection** does the same by hand, on whatever is
selected.

### How it avoids a seam

There is no seam, because nothing outside the hole is replaced. The model hands
back the unmasked part of the picture bit for bit, and only the masked pixels
are written — so there is no blending to get wrong and no feathering to tune.
A test asserts exactly that: not one pixel outside the selection may differ.

The model only ever sees 512 square. Rather than scale a whole frame down to
that and the answer back up — which would deliver every fill as a soft smear —
it is given a crop around the hole with half the hole's size again on each side
for context. Inpainting is entirely a question of what surrounds the hole, so
that context is not optional; and spending the 512 pixels on the crop rather
than the frame is what keeps the fill sharp.

### What it will and will not do

It continues what is around the hole: a bench, a wall, foliage, sky, a floor.
It is very good at that and it is *all* it does. It will not invent a thing
that was never there, because nothing here is prompted by words — there is no
text encoder in this pack and no diffusion model, both of which would take
minutes a picture on a processor rather than seconds.

So it is the honest form of the idea: **remove this** rather than **imagine
that**. A large hole in the middle of something detailed will come back soft,
which is the model admitting it does not know.

## Separating a picture by what is in it

```
open hillside.jpg
separate            # → separated into 3 layers: sky 49%, tree 40%, mountain 11%
```

Or **Layer ▸ Separate by Content…**, which lists what it found with the share
of the picture each takes, and lets you tick the ones worth a layer.

Each becomes an ordinary raster layer, named for what it is, holding that part
of the picture and transparent everywhere else — stacked above the one they
came from, which is left as it was. So the composite looks unchanged and the
picture is now something a layered editor can work on a piece at a time: grade
the sky without touching the hillside, clean the foliage and leave the
buildings alone.

`classes=` picks by name instead of by size, and `min=` sets how much of the
picture something has to be before it is worth a layer (2% by default).

### The boundaries are approximate

The model reasons on a 128-square grid whatever the picture's size, so on a
large frame one of its decisions covers a couple of dozen pixels. Its edges
follow the shape of a thing without hugging it.

Two things follow. The class scores are enlarged *before* the argmax rather
than after, which costs nothing and gives a boundary that follows an edge
rather than a grid. And `feather=` defaults to two pixels rather than zero,
because a soft edge is the honest way to draw a boundary the model was never
certain about.

For a real cut-out, this is the wrong tool and `segment` is the right one. The
two go together well: this says what is in the picture and roughly where, which
is exactly the prompt SAM wants.

## Enlarging

```
open small.jpg
upscale scale=2   # → enlarged 300x400 to 600x800, 1 layer through the model
                  #    in 6 tiles
```

Or **Image ▸ Upscale…**, which offers 1.5×, 2×, 3× and 4× and says how long it
will take.

The model only knows *four* times. Anything less is reached by asking it for
four and reducing afterwards, which is not the waste it sounds: the reduction
happens after the detail has been invented, so a request for two comes back
sharper than a model trained for two would have made it. The same reason
photographers oversample.

### Why it is not measured in decibels

Against a known original this scores **worse** than plain Lanczos:

```
lanczos 4x   29.39 dB
model   4x   24.62 dB
```

And it looks plainly better — sharper edges, legible fur, a hand with fingers
rather than a pink smudge. Both things are true at once, and the reason is that
PSNR rewards a blurred average everywhere over sharp detail in almost the right
place. A GAN upscaler invents detail that is *plausible* rather than *correct*,
which is the only thing it can do, since the detail is not in the file to
recover. Measured as high-frequency energy rather than error, the model's
output carries 84% more than Lanczos's.

That is worth stating plainly: **it makes up what it cannot know.** For a
photograph to look at, that is what anyone wants. For anything where the pixels
are evidence, it is not.

### What it does to the document

Enlarging changes the size of the picture, so it changes the document. It is
done in two halves that undo as one: an ordinary resize first, which knows how
to move a canvas, layer offsets, masks and the vector layers that have to be
redrawn rather than stretched — and then every raster layer's pixels replaced
with the model's, into the room the resize has already made. Nothing about the
geometry is written twice, and a document with type or shapes in it comes out
right without the enlarger knowing anything about them.

## What it does well, and what it does not

Cutting the detector's own answer out onto transparency, across the samples:

| picture | what it found | result |
|---|---|---|
| dog | dog 0.90 | the dog, cleanly |
| phone in hand | cell-phone 0.89, person 0.88 | the hand, or the phone |
| person in a field | person 0.83 | correct, and small |
| figure in a doorway | person 0.82 | correct, a silhouette |
| beach | chair 0.84 | the deckchair |
| building and forest | — | nothing it knows |
| old buildings | — | nothing it knows |
| path and plants | — | nothing it knows |
| road and mountain | — | nothing it knows |

Four of the nine come back empty, and that is the detector's list rather than
anything wrong with the pictures: sky, forest, mountain, road, building and
plant are not among the eighty classes. For those, the window is the way in —
the segmenter has no list and will separate whatever you click on.

The one thing worth knowing about box prompts: asked about a box, the model
answers "the object in this box". A dog sitting on a bench, boxed together, can
come back as one object. Alt-clicking the bench removes it.

## Notes on the implementation

The image embedding — nearly all of the cost — is cached by content hash, so
the first click on a picture takes about a second and every click after it is
immediate. That is what makes refining by clicking worth doing at all. The work
runs on its own thread so the window can say it is busy while it happens.

### The frame the encoder expects

This cost some time and is worth writing down.

The exported encoder does **not** take "the image resized so its long side is
1024". It ships a `config.yaml` saying `max_width: 1024, max_height: 682`, and
it means it: the image goes into that box, not a square.

Feeding it a portrait picture scaled to a 1024 long side put every mask 1.5
times too far down the frame — 1.5 being exactly 1024/682. The shape was right
and the placement was wrong, so a subject in the middle of the picture still
produced something that looked like a cut-out, and it survived several rounds
of looking at the results. What settled it was a synthetic image: a white
square at a known place, segmented, and the mask's bounds read off and compared
with where the square actually was. On real photographs the eye supplies the
benefit of the doubt; on a white square at y 300..500 a mask at y 450..749 is
simply wrong, and the ratio between them names the cause.

The decoder then wants `orig_im_size` given as that same frame rather than as
the picture, because it stretches its result across whatever size it is asked
for without first cutting off the padding the encoder added. Ask for the frame,
crop to the part the image occupies, and resize that — which is what the
reference implementation does, and what puts the mask where the object is.
