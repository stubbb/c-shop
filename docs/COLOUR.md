# Colour, profiles and ink

A pixel of `(220, 175, 143)` is not a colour. It is a colour once you know
which red, which green and which blue it is counting in — and a file that does
not say is guessed at. The guess is nearly always sRGB and nearly always right,
which is why an editor can go a long way without ever mentioning profiles.

It stops being right the moment a picture comes from a camera in Adobe RGB, or
has to leave for a press in CMYK. Then the difference is not subtle: the same
numbers, read against the wrong reference, are a different picture.

## The working space

A document works in one space, and everything arriving is converted into it.
That is what makes the rest of the program simple: within a document there is
only ever one answer to what a colour is, so blending, filtering and painting
never have to ask.

New documents work in sRGB. `Image ▸ Colour Profile…` shows what a document is
in and offers everything else the machine has, found by name rather than by
filename.

One thing to know before working in something other than sRGB: **the canvas
shows the working space's numbers directly.** There is no display transform, so
a document converted to Adobe RGB or a wider space is accurate in the file and
oversaturated on screen until it is exported back. That makes the wider spaces
useful for taking a picture through a conversion and out again, and not yet
useful for judging colour by eye. Which is where soft proofing would come in,
and it is not here either.

## Assign, or convert

The two things anyone wants to do here are opposites, and both are sometimes
right.

| | What happens | When |
|---|---|---|
| **Assign** | The pixels are untouched; the profile changes. The picture *looks different*, because the same numbers are now read as different colours. | A file arrived labelled wrongly, or not at all. |
| **Convert** | Every pixel is rewritten so the picture looks the same in the new space. The numbers change so the colours need not. | Moving a document between spaces on purpose, and what export does. |

Converting is not free. A colour the new space cannot reach is clipped to its
nearest neighbour, and at eight bits a channel the journey costs precision even
where nothing is clipped — see [what eight bits cannot
hold](#what-eight-bits-cannot-hold) below. Both are one undo step; converting
keeps the whole document to undo with, and says so in the History panel's
memory.

A document works in RGB, so `Colour Profile…` offers RGB profiles only. Ink is
made on the way out, not on the way in.

## Ink

A CMYK file is not a picture of colours. It is an instruction to a press: four
numbers saying how much of each ink to lay down, and what that comes to depends
entirely on the press, which is what the file's profile describes.

So there is exactly one correct way to open one — read the inks, read the
profile, and ask the profile what they print as. There is also a common wrong
way, which is to treat the four numbers as though they were already a colour.
That is what a print file gets from a program that does not know about
profiles, and it is why they so often arrive looking flat and dark.

Opening ink says so:

```
open press.tif   # → opened press.tif (32x32, 1 layer, four inks,
                 #    converted from Artifex CMYK SWOP Profile)
```

A CMYK file with no profile at all is a guess rather than a conversion, and the
report says that instead.

### Going back out

```
export plate.tif profile=/usr/share/color/icc/ghostscript/default_cmyk.icc
```

The result is four inks with the press's own profile embedded, so whoever opens
it next is not guessing either. It is a TIFF whatever the extension asked for,
because TIFF is the only format here that can hold ink.

Two things happen on the way that are worth knowing about.

**Transparency lands on paper.** Ink has no alpha channel, and paper is not
black, so anything short of opaque is composited onto white first.

**Black stops where the ink does.** A screen's black is out of a press's reach.
Sending `#000000` to a generic SWOP profile and opening the result gives
`#292828` — that is this program's own round trip, not an estimate — and it is
the profile telling the truth rather than losing something. A conversion that
returned pure black would be one that had ignored the profile.

### What is not here

Writing a CMYK JPEG. Four-component JPEG is a great deal of encoder for a
format no press asks for any more; CMYK JPEGs *open* — plenty exist — but ink
leaves as TIFF.

## Where profiles live in a file

Every container hides it somewhere different, and the decoders disagree about
finding it, so C-Shop reads the containers itself: a PNG `iCCP` chunk, a run of
JPEG `APP2` segments, TIFF tag 34675. A profile written by one program and read
by another goes missing often enough that a file can quietly lose the one thing
that says what its numbers mean.

On the way out the profile is embedded wherever the format has somewhere to put
it — PNG, JPEG, TIFF and WebP do; BMP, TGA, GIF and ICO do not, which is worth
knowing before choosing one for a picture whose colours matter.

C-Shop's own `.cshop` projects carry the working space too, in their own chunk,
and only when it is not the sRGB that everything assumes. A project written
before profiles existed still opens, and one written after still opens in a
build from before.

## What eight bits cannot hold

Round-tripping an ordinary colour through a wider space and back is accurate to
about a count:

```
(200, 175, 143) → Wide Gamut (189, 174, 145) → (201, 175, 143)
```

Near the edge of the gamut it is not:

```
(16, 243, 8)  → Wide Gamut (161, 225, 73) → (  0, 244, 6)
(20, 240, 10) → Wide Gamut (160, 222, 73) → ( 23, 240, 12)
```

One count apart on the way out, twenty-three counts apart coming home. Nothing
is wrong with the transform. A wide space spends its 256 steps over a much
larger volume, so sRGB's corners land where its own numbers are the small
difference between large ones, and quantising there is amplified on the way
back. Eight bits a channel is simply not enough to hold the journey.

This is measured rather than asserted — it is a test, so it stays true — and it
is the argument for working at sixteen bits where the journey matters.

## Sixty-four bits to a pixel

Sixteen bits a channel is not about seeing more colours. Nobody can tell one
sixteen-bit step from the next; eight bits is already finer than the eye at any
single boundary. It is about what happens *between* the file and the screen.

Take a gradient laid at thirty percent opacity — 256 tones squeezed into a
narrow band:

```
export band.png            # 78 distinct levels survive
export band.png depth=16   # 256 distinct levels survive
```

At eight bits the band has nowhere to put those 256 values and they collapse
into each other. That is what banding is, and no later step can undo it. The
same thing happens to a curve pulled hard, to a conversion out to a wider space
and back, and to half a dozen adjustments in a row: each one is fine, and the
sixth is visibly stepped.

**The compositor has always worked deeper than eight bits.** Layers are
composited in `Rgba16Float`, so every blend, opacity, adjustment layer and
effect is already evaluated with room to spare. Narrowing to eight bits was
simply the last thing that happened on the way out. `depth=16` asks it not to,
and the numbers above are what that is worth.

Save As offers it as a checkbox, shown only for the two formats that can hold
it — PNG and TIFF — and a scripted request to write sixteen bits into any other
is refused rather than quietly narrowed. Ink can be deep
too: `depth=16 profile=<a CMYK profile>` writes a sixteen-bit CMYK TIFF.

### Layers hold sixteen bits too

A raster layer stores either eight bits a channel or sixteen. A sixteen-bit
file opens deep, stays deep through a `.cshop` save and load, and exports deep
— bit for bit, every count, with nothing rounded on the way through:

```
open scan.png              # a sixteen-bit scan
info                       # → 4096x4096, 1 layers, 16 bits a channel, sRGB
export out.png depth=16    # identical to scan.png, sample for sample
```

`Image ▸ Mode` moves a document between the two, and `mode 8` / `mode 16` does
it from a script. Widening invents nothing — an eight-bit count becomes the
sixteen-bit count meaning the same fraction — so widening and narrowing again
gives back exactly the picture that went in. Narrowing on its own does lose
what eight bits cannot hold, and only undo has it on record.

### What is still eight bits

**The tools.** Painting, filters, adjustments, transforms, the paint bucket and
the gradient all work in eight bits. A sixteen-bit layer turns them away and
says so — naming the depth and the menu item that converts it — rather than
doing nothing and leaving you to wonder. Deep layers are for what arrives deep
and leaves deep: scans, raw conversions, renders, anything headed for a press.

**The compositor, past about eleven bits.** Layers are composited in
`Rgba16Float`, which is deeper than eight but shallower than sixteen: its
eleven bits of mantissa cannot tell adjacent sixteen-bit counts apart. The
better fit would be `Rgba16Unorm`, and wgpu does not permit it as a colour
attachment — only as a storage texture — so this is a floor, not a choice.

A document that is one full-canvas deep layer and nothing else therefore skips
the compositor entirely on the way out: there is nothing to composite, and
going through the GPU would cost bits it cannot carry. That is the shape a
photograph has from opening to export, so it is the common case rather than a
corner of one. Put a second layer over it and the export is composited, at
half-float, and the adjacent counts collapse — which is the honest trade and
is measured in the tests rather than assumed.

## From a script

| Command | |
|---|---|
| `profile` | Report the working space. |
| `profile assign PATH\|srgb` | Change what the numbers mean. |
| `profile convert PATH\|srgb` | Change the numbers, keep the appearance. |
| `export FILE profile=PATH` | Convert on the way out. A CMYK profile makes ink. |
| `export FILE depth=16` | Sixteen bits a channel. PNG and TIFF only; combines with `profile=`. |

`info` names the working space as well, so a script that reports on a document
does not need a second command to find out what its colours mean.
