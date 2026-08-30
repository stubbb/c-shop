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
