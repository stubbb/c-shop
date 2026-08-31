# Changelog

Versions rather than dates, because dates say when someone was at a keyboard
and versions say what the program is. It starts at 0.001 and every commit is
one increment, so a version here is a commit there and nothing is grouped,
smoothed or held back for a release.

Newest first.

- **0.072** — *A lamp that only ever adds light, and a README that is a list rather than an essay.* Relight gains **lighten only**: no pixel comes out darker than it went in, and ambient becomes a threshold instead of a darkener. The feature list moved to [FEATURES.md](docs/FEATURES.md) and the harness to [AGENTIC.md](docs/AGENTIC.md), leaving the README a page of one-liners. Twenty-two public functions nothing called were removed, along with two dependencies nothing used.
- **0.071** — *One picture, placed in several places, corrected once.* Smart-object sources moved onto the document, so several layers place one picture: replacing it reaches all of them at once, each at its own placement, and the file holds the picture once. Project format 2, with version 1 still opening.
- **0.070** — *Take the long work off the thread that draws the window.* One worker mechanism with a progress bar and a way to stop it, and everything slow through it. Resampling was made parallel on the way past — six times faster, bit for bit the same.
- **0.069** — *Show the document on the screen it is actually on, and let a pen press.* A colour-managed canvas and soft proofing, tablet pressure, brushes defined from a selection, and editable keyboard shortcuts.
- **0.068** — *Three things the depth already knew, a sky, and some skin.* Haze, depth of field and parallax from one depth map; sky replacement; skin smoothing that leaves eyes and hair alone.
- **0.067** — *Read the raw files that describe themselves.* DNG and the formats DNG-shaped enough to carry the same tags, developed to sixteen bits — including the lossless JPEG their sensor data is usually coded in.
- **0.066** — *Animations, SVG in and out, and a PDF page.* An animated GIF or APNG opens as a layer per frame with a timeline over it; SVG round-trips as editable geometry; PDF goes out as a page.
- **0.065** — *Geometry: bend the middle, keep what matters, and find the same scene twice.* Warp and puppet warp on one moving-least-squares engine, content-aware scale, perspective crop, and frame alignment with stacking.
- **0.064** — *Three ways of selecting that look at the picture.* Colour range, refine edge against the photograph's own boundary, and a selection traced back out as a path.
- **0.063** — *Three more ways to change your mind later.* Filters attached to a layer rather than run into it, masks that keep the path they were drawn from, and named layer states.
- **0.062** — *A layer that remembers the picture it was made from.* Smart objects: the placement is a setting, so the twentieth scaling is as good as the first and costs the history nine numbers.
- **0.061** — *Seven tools that reshape a picture rather than cover it.* Dodge, burn and sponge; blur, sharpen and smudge; the healing brush and its spot form; the history brush.
- **0.060** — *Let a layer hold sixteen bits, and stop losing them on the way out.* A raster layer holds eight bits a channel or sixteen, and `Image ▸ Mode` moves a document between them.
- **0.059** — *Remember something between one run and the next.* Window size, tool, brush, colours, panels, view settings and the last dozen files, as JSON that falls back to the defaults rather than failing.
- **0.058** — *Give the editor something to line things up against.* Rulers, guides, a grid, and snapping that catches by whichever edge is closest.
- **0.057** — *Write down what is missing, and what each absence would cost.* The roadmap: what the established editors offer, and whether each would earn its place here.
- **0.056** — *Stop the relighting drawing a black line round everything.* A depth model draws a cliff at an object's edge and lighting a cliff outlines it, so the shape is softened first and the slope capped.
- **0.055** — *Take the bearer token out of the command line.* A command line is world-readable through `/proc`; the server reads `CSHOP_TOKEN` from its environment instead.
- **0.054** — *Close two holes a security read turned up.* A connection ceiling, and the size of a request body checked before it is read.
- **0.053** — *Let every window that previews on the canvas be pushed aside.* Every one of them was a modal sitting over the middle of the picture it was previewing.
- **0.052** — *Drop four pictures the README never introduced.* They read as decoration that had wandered in; the files stay, where they are actually used.
- **0.051** — *Make masks, layers and selections convert into each other.* Depth as a mask, a greyscale layer as a mask on the one below, a mask as a selection with its softness kept.
- **0.050** — *Read the shape of a photograph, and light it again.* Depth Anything in the vision pack, and a lamp placed on a circle rather than in two numbers.
- **0.049** — *Fix two ways a window could do nothing and say nothing.* Separate by Content queued work and then closed itself before running it; the other one refused silently.
- **0.048** — *Make something disappear, by inventing what was behind it.* LaMa in the vision pack, filling a selection from what surrounds it — nothing outside the hole is touched.
- **0.047** — *Take a photograph apart by what is in it.* SegFormer on ADE20K, one layer per kind of thing, including the hundred and forty the detector has never heard of.
- **0.046** — *Enlarge a picture by inventing what a bigger sensor would have seen.* Real-ESRGAN, growing the whole document — canvas, layers, offsets and masks.
- **0.045** — *Show the denoiser what a real photograph looks like.* Gaussian noise added to a clean picture is not what a sensor does; a phone frame at high ISO is.
- **0.044** — *Take the noise out of a photograph, with a transformer.* SCUNet in the vision pack, run once with the strength mixed back afterwards so the question is settled against the finished picture.
- **0.043** — *Correct a lens: distortion, keystone, angle and vignette in one pass.* Composed into a single backward map, because every resampling pass costs sharpness.
- **0.042** — *Make the README say what the program currently is.* Every number in it re-measured rather than adjusted.
- **0.041** — *Offer the depth where someone saving a file would look for it.* A sixteen-bit checkbox in Save As, shown only for the formats that can hold it.
- **0.040** — *Sixty-four bits to a pixel, where they are worth having.* Sixteen bits a channel through export, for the room between the file and the screen rather than for colours nobody can see.
- **0.039** — *Read the colour a file says it is, and make ink of it on request.* ICC profiles read out of the containers directly, CMYK files opened as four inks, and `export profile=` sending a picture back out as ink.
- **0.038** — *Let the segmentation be grown as well as softened.* The model cuts a little inside a subject where the edge is soft, so Expand sits beside Feather.
- **0.037** — *Show the detect-then-segment example, and write it up for a caller that cannot see.* The illustration is drawn by the editor from the detector's own answer.
- **0.036** — *Put the masks where the objects are, and say when the model is working.* Every mask was landing too far down the picture — right shape, wrong place, which on a centred subject still looks like a cut-out.
- **0.035** — *Add a vision pack: find an object, and cut it out.* YOLOv8n and MobileSAM behind an optional install, in a process of their own so the editor keeps its single-binary build.
- **0.034** — *Add path editing: select and move anchors and handles.* A Direct Selection tool, with smooth anchors keeping their handles mirrored unless Alt says otherwise.
- **0.033** — *Add Bézier paths, a Pen tool, and boolean operations on shapes.* A path answers the same question every other shape already answered, so nothing downstream needed teaching.
- **0.032** — *Bound the undo stack by memory, and stop storing flat regions pixel by pixel.* Two hundred entries regardless of size was 275 MB on a 6000×6000 document.
- **0.031** — *Parallelise the flood fill by reducing rows to runs.* Whether a pixel matches does not depend on the fill's progress, so the picture can be reduced to runs a row at a time.
- **0.030** — *Make the bucket and the gradient fit the canvas they are given.* A gradient took 5.2 seconds on a 10000×10000 document and a bucket fill 3.9; four causes, all measured.
- **0.029** — *Store a selection where it has coverage, not across the document.* A marquee in one corner of a large canvas cost a hundred megabytes to hold.
- **0.028** — *Confine selection edits to the ring they can reach.* Feathering by three pixels worked from a blur over the whole canvas, when the edge it moves can only reach three pixels.
- **0.027** — *Stop editing costing what the canvas costs.* A stroke on a 10000×10000 document took 1.6 seconds; measured rather than guessed at, the GPU turned out to be innocent.
- **0.026** — *Add a desktop entry, so it launches like an application.* Icons at the eight sizes an icon theme looks for, installed under `~/.local/share` with no root.
- **0.025** — *Document deploying it.* DEPLOY.md: software Vulkan with the measurements behind it, fonts, the token, workspace ownership and sizing.
- **0.024** — *Run on a server, with or without a GPU.* The image carries Mesa's lavapipe, whose output is bit-identical to the hardware path.
- **0.023** — *Document serving, and why it is fenced the way it is.* SERVING.md: the tools, sessions, a worked session, and the reasoning behind each guard.
- **0.022** — *Serve the editor over MCP.* The script harness behind a socket, where a tool result can carry a picture of what it did.
- **0.021** — *Give a script a workspace it cannot leave, and a runner that outlives it.* The two things the editor needed before it could be served to anyone but the person sitting at it.
- **0.020** — *Document the style library, and show it.* The contact sheet is itself a script — nothing outside the editor assembled it.
- **0.019** — *Add fourteen styles, and make the two pencil ones scale.* Six tonal, five illustrative, one for type, and two more pencils.
- **0.018** — *Let a style scale itself to the image it is given.* A style whose radii are literal pixel counts means something different on every image it touches.
- **0.017** — *Document how the coloured-pencil style was arrived at.* One objective traced through the harness end to end, dead ends included.
- **0.016** — *Add a coloured-pencil style, and a place command to build it on.* With no path, `place` re-places the file the document was opened from, which is what lets a style lay an original back over its own treatment.
- **0.015** — *Correct the test count in the README.* It had drifted two features behind.
- **0.014** — *Add a style system, and two styles that use it.* A style is a named script fragment with holes in it — the same commands, parameterised, so there is one language to learn.
- **0.013** — *Say up front that there is an agentic harness.* It was only discoverable by scrolling past the feature list.
- **0.012** — *Merge the LLM intake pathway.* Driving the editor without a window, for something that can neither see nor click.
- **0.011** — *Document the scripted pathway with a worked job.* A real instruction end to end, showing what the report and `measure` are for.
- **0.010** — *Add a gradient command, and expand `~` in script paths.* Both turned up putting the pathway to work on a real job.
- **0.009** — *Add a scripted pathway for callers that cannot see or click.* Intake, draw, analyse, return.
- **0.008** — *Add clipboard support: copy, cut and paste.* Including Copy Merged and Paste in Place, carrying a feathered edge with them.
- **0.007** — *Note in the README that this is a personal tool in active development.* Expectations before the feature list.
- **0.006** — *Add a layered project format and PSD import and export.* Two ways to keep a document rather than a picture of one.
- **0.005** — *Bring the README up to date.* Test count, line count and startup figure re-measured.
- **0.004** — *Make the Layer Style dialog movable, and give shadows a little spread.* A dialog whose preview is the canvas is no use sitting over the middle of it.
- **0.003** — *Add gradient and pattern overlays.* Two more layer effects, in the overlay band above Colour Overlay.
- **0.002** — *Add layer effects.* Eight of them, rendered from a layer's alpha and composited around it.
- **0.001** — *Initial release of C-Shop.* A native, GPU-accelerated, layer-based image editor: a single Rust binary with a Vulkan compositor and no browser, Electron or web view anywhere.
