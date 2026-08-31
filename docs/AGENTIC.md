# Driving it without a window

The same editor, with no window and no pointer — for batch work, for tests, and
for callers that cannot see a canvas or click a button. Nothing here is a
second implementation: a script drives the identical code the interface drives,
so anything the program can do it can be asked to do this way.

Three layers, each built on the one before:

| | |
|---|---|
| **A script** | One command per line, run in this process. `cshop --run '…'` or `cshop --script file`. |
| **A server** | The same harness behind a socket, speaking [MCP](https://modelcontextprotocol.io), where a result can carry a picture of what it drew. `cshop --serve` |
| **A container** | The server on a machine with no GPU, rendering in software at no measurable cost for this workload. `docker compose up -d` |

## A script

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

![The style library](style-showcase.jpg)

## Over a network

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
checked. [SERVING.md](SERVING.md) has the details, the worked session
and the reasoning. No HTTP or JSON dependency was added for any of it; both are
in the tree, like the project format and the PSD codec.

## On a server

```sh
docker compose up -d
```

The image carries a software Vulkan driver, so it renders on a machine with no
GPU — and for this workload that costs nothing measurable, because the time
goes on CPU-side filtering and encoding rather than on compositing. Pass
`--gpus all` and it will use a real one instead, unchanged.
[DEPLOY.md](DEPLOY.md) covers fonts, the token, workspace ownership
and sizing, with the measurements behind that claim.


## What it is not

It is not a macro recorder and not a plugin API. A script is the editor's own
vocabulary written down, so there is one language to learn and one place a
capability lives — which is also why a style is a script fragment rather than a
new kind of object.

It cannot see. `measure` and the run report exist because of that: a caller
places type by asking what size it will be rather than by looking at what it
came out as, and reads back where every layer landed rather than inspecting a
picture. [SCRIPTING.md](SCRIPTING.md) has the full command reference, two
worked examples of an agent taking a photograph from an instruction to a
finished image, and an appendix tracing how one style was arrived at — dead
ends included.
