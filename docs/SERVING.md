# Serving C-Shop over MCP

The script harness lets something that cannot see a canvas drive the editor.
This puts that harness behind a socket, so the caller need not be on the same
machine — and, more to the point, so a tool result can carry a **picture**.
That is what closes the loop the harness was built for: describe, draw, look,
correct.

```sh
cshop --serve --workspace ~/pictures
```

```
serving MCP on http://127.0.0.1:7333/mcp
workspace: /home/you/pictures
no token; reachable only from this machine
```

The server speaks the [Model Context Protocol](https://modelcontextprotocol.io)
over HTTP: JSON-RPC 2.0 at `POST /mcp`, protocol revision `2025-06-18` (with
`2025-03-26` and `2024-11-05` also accepted). There is no SDK behind it and no
async runtime — the HTTP and the JSON are both in this repository, for the same
reason the project format and the PSD codec are.

## Talking to it

```sh
curl -s localhost:7333/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

`GET /` prints a summary, and `GET /health` answers with the version, the
protocol revisions, the workspace and the sessions currently open — which is
what a monitor or a load balancer wants, and needs no token.

For a client that reads a config file, the shape is the usual one:

```json
{
  "mcpServers": {
    "cshop": {
      "type": "http",
      "url": "http://127.0.0.1:7333/mcp"
    }
  }
}
```

## The tools

| Tool | What it does |
|---|---|
| `run_script` | Run a script and report, optionally with a picture of the result. This is the editor. |
| `render` | Look at the document as it stands, without changing it. |
| `list_styles` | The style library, with the parameters each style takes. Name one to see how it is built. |
| `describe` | The command reference: `commands`, `syntax`, `filters`, `adjustments`, `effects`, `blends`, `fonts`. |
| `workspace` | What may be opened, and where output goes. |
| `reset` | Close a session's document. |

Five of the six exist because of one problem: a caller arriving over a socket
has the tool descriptions and nothing else. It cannot use `run_script` well
until it knows what styles exist, what commands there are, and what files it
may open — so it can ask, rather than guess.

`run_script` is deliberately the whole editor rather than forty tools with one
command each. The script language already *is* the interface an agent wants,
and splitting it up would only make a worse copy of it: a caller would lose the
ability to say "do these nine things to this document" in one call, and every
command would need its own schema to drift out of date.

## Sessions

A session is one document, open across several calls. Holding it is most of the
value of serving at all — the GPU context costs far more than a typical script,
and a caller that opens a photograph, looks at it, and then adjusts it needs the
photograph still to be open on the second call.

Clients that manage sessions send back the `Mcp-Session-Id` handed to them at
`initialize`. A caller can also pass `session` to any tool to name its own.
Sessions expire after thirty idle minutes, and thirty-two is the most that will
be held at once — past that the least recently used is dropped, so a client that
has lost track of its ids cannot wedge the server.

## What a session looks like

Opening a photograph, styling it, measuring type before placing it, and
exporting — each step a separate call, with a look in between.

```
run_script  open garden.jpg
            resize fit=1400
            style watercolour
→ "garden.jpg: 1052x1400, 1 layers … 14 steps ran, 0 failed"  + a picture

run_script  measure text "Garden Walk" family="C059" size=120 bold
→ 'measure "Garden Walk": 815x144'

            # 815 wide in a 1052 frame centres at x = 118
run_script  text 118 1240 "Garden Walk" family="C059" size=120 bold color=#d8b45a
            style gilded-lettering size=4
→ layer [1] Garden Walk  Type  fx: Gradient Overlay, Bevel & Emboss, Drop Shadow  + a picture

run_script  layer flatten
            export garden-walk.jpg quality=92
```

The measurement step is the one worth copying. Type gets placed from its real
size rather than from a guess, which is the difference between centred and
nearly centred.

The picture that comes back is scaled — 768 pixels on its longest side by
default, `image_fit` to change it, 2048 at most. That is a deliberate ceiling:
images travel as base64 inside a JSON string, which costs a third again on top
of the PNG. **Judge fine texture on a full-size `export`, not on the returned
image.** Hatching, grain and banding all survive a downscale that they do not
survive at size — that mistake has been made twice in this repository already.

## What stands between the port and the filesystem

The script language can read and write files. Served over a socket that is a
filesystem primitive with a port in front of it, so three things guard it, and
all three are on by default.

**A workspace.** Every path a script names resolves inside one directory and
cannot leave it. There is no flag to turn this off. Two checks, because either
alone can be walked around:

- Lexically, `..` and absolute paths and `~` are refused outright.
- Canonically, the deepest part of the path that exists is resolved and must
  still be inside the root.

Only the pair is worth anything. The lexical check cannot see a symlink; the
canonical check cannot see a file that does not exist yet — which every export
target is. A symlink planted in the workspace and pointing out of it is
lexically an ordinary relative path, and is refused by the second check alone.
The `workspace` listing marks such entries rather than offering them.

**Loopback by default.** `--serve` binds `127.0.0.1:7333`. Serving anywhere
else has to be asked for, and asking for it requires `--token` — a server
reachable from the network without one **refuses to start**:

```
$ cshop --serve 0.0.0.0:7333 --workspace ~/pictures
refusing to serve on 0.0.0.0:7333 without --token.
This server can read and write files in its workspace, so exposing it beyond
localhost without one would hand that to anyone who can reach the port.
Either pass --token SECRET, or bind to 127.0.0.1.
```

Set the token in the environment rather than on the command line:

```sh
export CSHOP_TOKEN="$(openssl rand -hex 24)"
cshop --serve 0.0.0.0:7333 --workspace ~/pictures
```

A command line is world-readable through `/proc`, so `--token` puts the secret
where every account on the machine can read it for as long as the server runs.
An environment block is readable only by its owner. The flag still works and
still wins, but it says why it is the wrong door.

The editor removes `CSHOP_TOKEN` from its own environment once it has read it,
so the model sidecar — which would otherwise inherit it — never sees it.

With a token, `Authorization: Bearer SECRET` is required on `/mcp`, compared in
constant time. `/health` stays open so a monitor need not hold the secret.

**An origin check.** A page in a browser can post to localhost. Without this,
any site the operator visited could drive their editor. Requests carrying an
`Origin` that is not a loopback one are refused with 403; `--allow-origin` adds
others. A request with no `Origin` at all is not from a browser and is left
alone.

Beyond those: header and body sizes are capped, reads time out, session ids are
drawn from `/dev/urandom` rather than a counter, and a connection that stops
making sense is dropped rather than reasoned with.

There is deliberately **no** TLS here. Terminate it in front — a reverse proxy
does that job better than this would, and it is not the sort of thing to
hand-write next to the JSON parser.

```sh
cshop --serve 0.0.0.0:7333 --token "$(openssl rand -hex 32)" --workspace /srv/pictures
```

## Options

```
--serve [ADDR]          host:port, a bare port, or nothing (127.0.0.1:7333)
--workspace DIR         the only directory scripts may touch (default: cwd)
--token SECRET          require Authorization: Bearer SECRET; prefer
                        the CSHOP_TOKEN environment variable
--allow-origin ORIGIN   permit a browser origin besides localhost
```

## How it is put together

| | |
|---|---|
| `mcp/http.rs` | Enough HTTP/1.1 to carry it: a blocking accept loop, a thread per connection, keep-alive, and every limit a socket facing a network needs. |
| `mcp/json.rs` | A parser and a writer. Objects keep their insertion order, and nesting is bounded so that a few kilobytes of `[[[[…` is an error rather than a stack overflow. |
| `mcp/protocol.rs` | JSON-RPC framing and the protocol's own methods. |
| `mcp/tools.rs` | The six tools and their schemas. |
| `mcp/reference.rs` | The manual. Blend modes and font families are read from the code and from the machine, so it cannot describe an editor that no longer exists. |
| `mcp/editor.rs` | The one thread that owns the GPU and the open documents. |
| `mcp/server.rs` | Binding, and who is allowed to talk. |

A thread per connection, with all the editing funnelled onto one thread behind
a channel. That is not a compromise made to satisfy the borrow checker —
serialising the work is what we want. There is one GPU, and two requests
compositing into the same document at once would race whatever they disagreed
about, producing something that looked like a rendering bug rather than what it
was.
