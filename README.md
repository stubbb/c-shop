# C-Shop

A native, GPU-accelerated, layer-based image editor. No browser, no Electron,
no web view — a real desktop binary that composites on the GPU and is on screen
in about 400 ms.

It also has an **agentic harness**: the same editor drives from a script with
no window at all, so something that cannot see a canvas or click a button can
still edit images with it — placing type from measurements and reading back a
report of what it drew. It runs as an **MCP server** too, so that harness can
be reached over a network, with each result carrying a picture of what it did.

![C-Shop](docs/screenshot.png)

## Status

A personal tool, in active development. It is built for its author's own work,
so it changes whenever a bug turns up or a feature is wanted, and those changes
are pushed as they happen rather than gathered into releases. Expect no version
numbers and no stability promises — though the project format carries a version
and skips chunks it does not recognise, so files already saved keep opening.

## Why

Most capable image editors are either enormous proprietary suites or web apps
wearing a desktop costume. C-Shop is neither: a single Rust binary, a Vulkan
compositor, and an interface that behaves the way people who edit images
already expect — layers, masks, blend modes, non-destructive adjustments, and
the keyboard shortcuts your hands already know.

It needs no system packages beyond a working GPU driver. X11 is opened through
`dlopen`, Vulkan through `ash`, and every remaining dependency is pure Rust, so
`cargo run --release` is the whole install.

## What it does

**Layers.** Raster layers, nestable groups, fill layers, re-editable type and
vector shape layers, adjustment layers. Layer masks and clipping masks,
per-layer opacity and fill opacity, four lock modes, drag-to-reorder, and 27
blend modes (plus Pass Through for groups) evaluated on the GPU and checked
against a CPU reference to within 0.84 of one 8-bit level.

**Selections.** Rectangular and elliptical marquees, freehand and polygonal
lassos, magic wand. All four boolean modes; feather, expand, contract, border,
smooth, invert, grow and similar; animated marching ants. Every tool respects
the selection, including partial coverage along a feathered edge.

**Masks and channels.** Layer masks from a selection or blank, painted on
directly, enabled, applied or deleted. Quick Mask. Selections saved to alpha
channels in the Channels panel.

**Painting.** Brush, pencil, eraser and clone stamp sharing one stroke engine —
size, hardness, opacity, flow and spacing behave identically across all four.
Paint bucket with tolerance and contiguity; gradients in five shapes with
editable stops.

**Type.** Re-editable text layers rendered from your installed fonts, with
point text and wrapping paragraph boxes, real or synthesised bold and italic,
alignment, leading, tracking and anti-aliasing. Edited live on canvas with a
caret; a whole typing session is a single undo step.

**Layer effects.** Drop shadow, outer glow, bevel and emboss (inner, outer,
emboss and pillow), inner shadow, inner glow, satin, colour overlay, gradient
overlay, pattern overlay and stroke — each with its own blend mode, opacity,
colour and geometry, and a shared global light. Every effect is a function of
how far a pixel sits from the layer's edge, so one distance field drives all of
them. Fill opacity scales the layer's own pixels and not its effects, which is
what makes a stroke-only layer possible. The Layer Style window applies as you
work and can be dragged aside, so the canvas is the preview.

**Shapes.** Rectangles, rounded rectangles, ellipses, polygons, stars and
lines, drawn from signed distance fields so fill and stroke stay perfectly
registered. Fill and stroke are independent, the stroke sits inside, centred or
outside, and every shape stays editable after it is drawn.

**Adjustments.** Brightness/Contrast, Levels, Curves, Exposure, Vibrance,
Hue/Saturation, Color Balance, Black & White, Channel Mixer, Photo Filter,
Invert, Posterize, Threshold and Gradient Map — each available destructively
through a dialog with a live preview *and* as a non-destructive adjustment
layer, with a monotone curve editor and a live histogram.

**Filters.** Thirty of them: Gaussian, Box, Motion, Radial, Surface and Average
blur; Sharpen and Unsharp Mask; Add Noise, Median and Dust & Scratches; Twirl,
Pinch, Spherize, Wave and Polar Coordinates; Mosaic, Crystallize and Fragment;
Clouds and Fibers; Find Edges, Emboss, Solarize and Diffuse; High Pass, Offset,
Maximum, Minimum and a 5×5 custom convolution. Each previews live, with zoom
and pan so you can judge detail at 1:1, and repeats with `Ctrl+F`.

**Transforms.** Free Transform with eight handles and rotation, where Shift
constrains, Alt works from the centre and Ctrl pulls a corner into a true
perspective distort. Fixed rotations and flips, Crop with aspect presets, Image
Size with four resampling filters, Canvas Size with a nine-way anchor.

**Clipboard.** Copy, Cut, Copy Merged, Paste and Paste in Place, carrying a
feathered selection's soft edge with it. Images go to and come from the system
clipboard, so a copy here pastes into other programs and theirs paste in here.

**Files.** A native layered project format, `.cshop`, that keeps the whole
document — the layer tree, groups, masks, adjustment settings, live type and
shape descriptions, effects and saved channels — still editable when reopened.
**PSD** import and export carries layers, groups, masks, opacity, blend modes,
clipping and visibility both ways, plus the flattened composite other programs
read. Flat formats: PNG, JPEG, BMP, TIFF, TGA, GIF, WebP and ICO.

## Build and run

Rust 1.85 or newer, and a GPU driver with a Vulkan, Metal or DX12 backend.

```sh
git clone https://github.com/stubbb/c-shop.git
cd c-shop
cargo run --release
```

Open files straight from the command line:

```sh
cargo run --release -- photo.jpg
```

Render a frame offscreen without a window, which is how the interface is
checked in CI:

```sh
cargo run --release -- --screenshot out.png --demo --size 1500x900
```

`--help` lists the rest. [docs/SHORTCUTS.md](docs/SHORTCUTS.md) is the full
keyboard reference.

To put it in the desktop's application menu, with an icon and the file types it
opens:

```sh
./packaging/install-desktop.sh          # --no-desktop to skip the desktop icon
./packaging/install-desktop.sh --uninstall
```

Everything it writes goes under `~/.local/share`, so it needs no root. It also
registers the `.cshop` format — matched by the file's own magic rather than its
extension — so a project file gets an icon and something to open it with.

It can also be driven without a window at all, by a script rather than a
pointer — for batch work, for tests, and for callers that cannot see:

```sh
cshop --run 'new 400 240 background=#20304a
text 40 154 "Hello" size=54 color=#ffffff bold
effect drop-shadow distance=6 size=8
export out.png'
```

Every run answers with a report of where each layer landed and what failed,
and `measure` sizes text before anything is drawn — so a caller that cannot
see the canvas can still place things by number rather than by guessing.
Named **styles** — parameterised script fragments — package a look so it can be
applied to anything: `style watercolour`, `style noir shadows=0.7`. Seventeen
ship, each with its reasoning written down beside it, and they scale themselves
to whatever size of image they are handed.
[docs/SCRIPTING.md](docs/SCRIPTING.md) has the command reference, two worked
examples of an agent taking a photograph from an instruction to a finished
image, and an appendix tracing how one of those styles was arrived at — dead
ends included.

![The style library](docs/style-showcase.jpg)

| | | | |
|---|---|---|---|
| ![Before](docs/example-garden-before.jpg) | ![After](docs/example-garden-after.jpg) | ![Pencil sketch](docs/example-sketch-after.jpg) | ![Coloured pencil](docs/example-coloured-after.jpg) |

### Over a network

The same harness serves over [MCP](https://modelcontextprotocol.io), so a
caller elsewhere can drive the editor and — the point of it — **see what it
drew**, since a tool result can carry an image:

```sh
cshop --serve --workspace ~/pictures
```

A document stays open between calls, so a picture is built up in steps with a
look in between rather than composed blind. Six tools: `run_script` is the
whole editor, and the other five are how a caller arriving cold finds out what
styles exist, what the commands are, and which files it may open.

Because a script can read and write files, a served editor is confined: every
path resolves inside one workspace and cannot leave it, the socket is loopback
unless a token is set — it refuses to start otherwise — and browser origins are
checked. [docs/SERVING.md](docs/SERVING.md) has the details, the worked session
and the reasoning. No HTTP or JSON dependency was added for any of it; both are
in the tree, like the project format and the PSD codec.

### On a server

```sh
docker compose up -d
```

The image carries a software Vulkan driver, so it renders on a machine with no
GPU — and for this workload that costs nothing measurable, because the time
goes on CPU-side filtering and encoding rather than on compositing. Pass
`--gpus all` and it will use a real one instead, unchanged.
[docs/DEPLOY.md](docs/DEPLOY.md) covers fonts, the token, workspace ownership
and sizing, with the measurements behind that claim.

### Recognising and cutting out

An optional pack adds two neural networks — one that finds objects and says
where they are, one that turns a point or a box into a mask:

```sh
vision/setup.sh
```

**Select ▸ Segment Object…** opens a window where clicking the thing you want
makes it the selection; click again to refine, Alt-click to exclude, and a
slider softens the edge. From a script the two work in sequence — `detect` to
find a dog, `segment` to cut it out — and the result is an ordinary selection,
so everything the editor already does with one applies.

The models run in a separate process, so the editor keeps its single-binary,
no-dependency, offline build whether they are installed or not.
[docs/VISION.md](docs/VISION.md) covers what they do well and where they miss.

## Interface

| | |
|---|---|
| ![Shapes](docs/screenshot-shapes.png) | ![Type](docs/screenshot-type.png) |
| Vector shape layers, Bézier paths and boolean operations | Re-editable type |
| ![Curves](docs/screenshot-curves.png) | ![Filters](docs/screenshot-filter.png) |
| Adjustments with a live histogram | Filters with a zoomable preview |
| ![Layer effects](docs/screenshot-effects.png) | ![Selections](docs/screenshot-selection.png) |
| Layer effects | Selections and masks |

## How it is built

Five crates, with dependencies pointing one way:

```
cshop-app  →  cshop-ui  →  cshop-gpu  →  cshop-core
                       ↘   cshop-io   ↗
```

- **`cshop-core`** — the document model and every pixel operation, with no GPU
  and no interface. Blend modes, adjustments, filters, selections, masks,
  paint, resampling, text layout, shape rasterisation, undo history.
- **`cshop-gpu`** — the Vulkan compositor: layer textures, the ping-pong
  render passes that evaluate blend modes, and readback.
- **`cshop-io`** — image decoding and encoding.
- **`cshop-ui`** — panels, tools, dialogs, the theme, and the whole
  interaction layer.
- **`cshop-app`** — the window, the swapchain, and the offscreen capture path.

Around 38,000 lines of Rust and 500 of WGSL. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
explains the decisions that are not obvious from the code — the colour space,
why the compositor cannot use fixed-function blending, and how vector layers
avoid special-casing everything downstream.

## Testing

634 tests, and the interesting ones are not unit tests:

- **GPU against CPU.** Every blend mode and adjustment is implemented twice,
  once on each, and the two are compared pixel by pixel. Worst divergence:
  0.84 of one 8-bit level for blending, 1.1 for adjustments.
- **Synthetic input through the real interface.** A headless harness drives
  `CShopApp::update` with real pointer and keyboard events, so a widget that
  covers another fails here rather than in someone's hands. Three input
  regressions had shipped before it existed.
- **Offscreen rendering.** The `--screenshot` path exercises the identical
  egui and compositor pipeline as the window, so the interface can be looked
  at in CI.
- **Round trips and damaged files.** Both layered formats are written and read
  back and compared property by property, and both are fed truncated and
  bit-flipped versions of their own output — which must be refused, never
  crash.
- **Every style, discovered rather than listed.** The style library is read off
  disk and each one applied, so a style added later is covered without anyone
  remembering to add a test for it.
- **Cost that must not follow the canvas.** Editing a 10000x10000 document
  should cost what editing a small one does, so those are timed against each
  other rather than against a stopwatch — the shape of the cost is the thing
  being tested, not the speed of the machine.
- **The server, over a real socket.** The MCP tests bind a port and speak HTTP
  to it, because most of what could go wrong there is in the transport and in
  the guards around it — neither of which a test that calls the handler
  directly can see. The sandbox tests plant a symlink out of the workspace and
  confirm it is refused.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

## Not there yet

Custom pattern tiles loaded from an image (the pattern overlay draws six
generated figures), boolean shape combining, and selecting a range within a
text layer. PSD carries layers as raster: type and shapes are
flattened on the way out and 16-bit and CMYK files are refused rather than
misread.
The toolbar is complete: every tool it shows is implemented.

## Licence

MIT OR Apache-2.0, at your option.
