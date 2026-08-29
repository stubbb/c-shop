# Running C-Shop on a server

```sh
docker compose up -d
curl -s localhost:7333/health
```

That is the whole thing. What follows is why it is put together the way it is,
and what to change when the server is not a laptop.

## It runs without a GPU

The editor composites on the GPU, and a server usually has none. The image
carries Mesa's **lavapipe**, a software Vulkan driver, so there is always an
adapter to render on.

This is not a degraded path bolted on for containers. The compositor's output
on `llvmpipe (Cpu, Vulkan)` is **bit-identical** to a discrete GPU's — measured
across a full styled render, worst channel difference zero of 255 — which is
what the GPU-against-CPU tests in the suite exist to keep true.

It is also, for this workload, no slower. Measured through the server, same
machine, best of three, an RTX 4060 against llvmpipe on 16 cores:

| workload | GPU | software | |
|---|---|---|---|
| `duotone`, 900px | 488 ms | 503 ms | 1.03× |
| `noir`, 1400px | 689 ms | 717 ms | 1.04× |
| `watercolour`, 900px | 2039 ms | 2052 ms | 1.01× |
| `poster-print`, 1400px | 3585 ms | 3521 ms | 0.98× |
| 30 Overlay layers, 2000px | 3233 ms | 3244 ms | 1.00× |

That is not because llvmpipe is as fast as a 4060 at compositing. It is because
compositing is not where a served request spends its time. Filters and
adjustments run on the CPU across every core, and around them sit decode,
resampling, read-back and PNG encode — all CPU. The GPU pass is real work but a
small share of the total, so removing it changes little.

Which means a GPU is worth passing through for the *interactive* editor, where
the compositor runs every frame with no filtering between frames, and is worth
very little for a server. Plan for cores, not for a card.

If the host does have one, pass it through and it will be used instead, with no
change to the image:

```sh
docker run --gpus all -p 127.0.0.1:7333:7333 -v "$PWD/workspace:/workspace" cshop
```

That needs the NVIDIA Container Toolkit on the host. The image already asks for
the right driver capabilities, so nothing else is required. Check which adapter
was chosen in the logs:

```
GPU: llvmpipe (LLVM 15.0.7, 256 bits) (Cpu, Vulkan)      ← software
GPU: NVIDIA GeForce RTX 4060 (DiscreteGpu, Vulkan)       ← passed through
```

## Fonts are not optional

Type is drawn from the families actually installed, and a slim image has none —
so `text` would fail on the server and nowhere else, which is the worst way to
find out. The image installs DejaVu, Liberation and the URW base-35 set, which
is what provides the families the documentation uses: `C059`, `P052`, `Z003`,
`Nimbus Sans`, `Nimbus Roman`, `URW Bookman`, `URW Gothic`.

Ask the running server what it actually has:

```sh
curl -s localhost:7333/mcp -H "Authorization: Bearer $CSHOP_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"describe","arguments":{"topic":"fonts"}}}'
```

To add your own, mount them where the scan looks:

```yaml
volumes:
  - ./fonts:/usr/share/fonts/truetype/custom:ro
```

## The token

A container has to bind `0.0.0.0` to be reachable at all, which is exactly the
case the editor [refuses to serve](SERVING.md) without a token — rightly, since
a script can read and write files in its workspace.

So the entrypoint **generates one and logs it** when `CSHOP_TOKEN` is unset:

```
-----------------------------------------------------------------
 No CSHOP_TOKEN was set, so one was generated for this container:

     3f9a1c…

 Send it as:  Authorization: Bearer 3f9a1c…
-----------------------------------------------------------------
```

Fine for a try; set your own as soon as anything has to reconnect across a
restart. The compose file requires it rather than defaulting, so it cannot be
forgotten by accident:

```sh
export CSHOP_TOKEN="$(openssl rand -hex 24)"
docker compose up -d
```

There is deliberately no way to turn the token off for a network-facing bind.
Publishing to loopback only — which the compose file does — means nothing off
the host can reach it regardless.

## The workspace is the boundary

Whatever is mounted at `/workspace` is both what scripts may open and where
their output lands. They cannot reach anything else: paths are confined by the
sandbox described in [SERVING.md](SERVING.md), and the container runs as a
non-root user that owns only that directory.

```yaml
volumes:
  - ./workspace:/workspace
```

Mount read-only inputs separately if you want them protected from the editor
itself:

```yaml
volumes:
  - ./originals:/workspace/originals:ro
  - ./output:/workspace/output
```

## Configuration

| Variable | Default | |
|---|---|---|
| `CSHOP_TOKEN` | generated, logged | Bearer token required on `/mcp`. |
| `CSHOP_ADDR` | `0.0.0.0:7333` | Where to bind inside the container. |
| `CSHOP_WORKSPACE` | `/workspace` | The only directory scripts may touch. |
| `CSHOP_ALLOW_ORIGINS` | — | Comma-separated browser origins to permit. |
| `RUST_LOG` | `cshop=info,warn` | |
| `WGPU_BACKEND` | `vulkan` | Pinned so it cannot quietly fall back to a slower GL path. |

Arguments after the image name replace the whole command line, so the image is
also a way to run one-off work:

```sh
docker run --rm -v "$PWD:/workspace" cshop --run 'open in.jpg
style noir
export out.jpg'
```

## Sizing

Cores buy throughput, for the reason above: the work is CPU-bound whether or not
there is a GPU, and the filters are parallel across every core they are given.
The numbers in the table came from 16; expect the filter-heavy styles to scale
roughly with what you allow.

Memory is bounded by the documents held open. The editor budgets 512 MB of
textures per session on a software adapter, sessions expire after thirty idle
minutes, and thirty-two are held at most. 4 GB and 4 cores is comfortable for
one operator working at photographic sizes.

Requests are serialised onto one editor thread by design — there is one adapter,
and two requests compositing into the same document would race whatever they
disagreed about. So scaling past one busy caller means more replicas, not a
bigger container. Each replica needs its own workspace if they are to write
without colliding.

Watch what it is holding:

```sh
curl -s localhost:7333/health | python3 -m json.tool
```

## In front of it

Terminate TLS in a reverse proxy. There is none in the server, deliberately —
a proxy does that job better than a hand-written one next to a hand-written
JSON parser would.

```nginx
location /mcp {
    proxy_pass http://127.0.0.1:7333;
    proxy_read_timeout 300s;   # a large render is not a hung request
}
```

The read timeout is the setting that matters. A style built on a wide surface
blur takes seconds at web sizes and tens of seconds at print sizes — on a GPU
just as much as without one — and a proxy with a 60-second default will cut one
off and report something that looks like a crash.

`/health` needs no token, so a load balancer or `HEALTHCHECK` can use it without
holding the secret. The image already has one wired to it.

## Building it

```sh
docker build -t cshop .
```

The build uses BuildKit cache mounts for the cargo registry and the target
directory, so a rebuild after a source change is quick. It ends by rendering a
small image with type on it and failing if that does not work — a missing
driver or a missing font is then a failed build rather than a failed request in
production a week later.
