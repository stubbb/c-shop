# The vision pack

Two neural networks, as something you opt into.

```sh
vision/setup.sh
```

That is the whole installation: a Python environment and about 60 MB of model
weights, under `~/.cache/cshop/vision`. Nothing in the editor needs it — open,
edit and save behave exactly as before whether it is there or not — and only
two commands, `detect` and `segment`, and one window ask for it.

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
| `segment … feather=2` | Soften the edge of the result, in pixels. |

With nothing said, `segment` uses whatever `detect` last found — which is the
whole point of running them in sequence.

## From the window

**Select ▸ Segment Object…** opens a window that is not modal, because the
canvas is its control as well as its preview. Click the thing you want; the
selection appears. Click again to refine, Alt-click to say "not this", and drag
the feather slider to soften the edge. *Find objects* lists what the detector
recognises, if anything, so a dog can be chosen by name instead of by aim.

Cancel puts the selection back as it was.

## What it does well, and what it does not

Measured on the sample images, cutting the named subject out onto transparency:

| picture | subject | prompt | result |
|---|---|---|---|
| dog | dog | `class=dog` | clean, though it keeps the bench the dog sits on |
| phone in hand | phone | `class=cell-phone` | clean, hand included |
| old buildings | building | a point | clean |
| beach | beach | a point | the sand, correctly |
| road and mountain | road | a point | the road surface |
| person in a field | person | `class=person` | correct, and small |
| building and forest | building | a point | took the hillside instead |
| path and plants | path | a point | took a leaf instead |

The last two are not model failures; they are aiming failures. A point in a
large scene selects whatever region it lands in, and choosing that point from a
thumbnail is guesswork. That is the argument for the window over the script:
you click where you mean, see what you got, and refine — which is two seconds,
against however long it takes to guess a coordinate.

The dog keeping its bench is a different thing, and worth understanding.
Prompted with a box, SAM answers "the object in this box"; a dog sitting on a
bench, boxed together, is one object as far as it is concerned. Alt-clicking
the bench removes it.

## Notes on the implementation

The image embedding — nearly all of the cost — is cached by content hash, so
the first click on a picture takes about a second and every click after it is
immediate. That is what makes refining by clicking worth doing at all.

One thing was wrong for a while and is worth recording. The exported decoder
returns its mask over the *padded square* the encoder works in and stretches it
across whatever size it is asked for, without first cutting the padding off.
Passing the picture's own size therefore squashed every mask into two thirds of
its width, with a hard vertical edge exactly where the padding began. It looked
plausible on a subject in the middle of the frame, which is how it survived
several tests. The mask's right edge measured 0.665 of the width where the
image occupied 0.667 of the square — that is what gave it away.
