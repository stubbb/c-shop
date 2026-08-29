# C-Shop as a server.
#
# The editor composites on a GPU, and a server usually has none — so this image
# carries a software Vulkan driver (Mesa's lavapipe) and runs on the CPU. That
# is not a degraded mode bolted on for containers: the compositor's output on
# llvmpipe is bit-identical to a discrete GPU's, which is checked by the
# GPU-against-CPU tests in the suite. It is slower, and that is the whole of the
# difference. Pass a real GPU through with `--gpus all` and it will be used
# instead, with no change to this image.

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS build
WORKDIR /src

# The whole tree, because the binary embeds assets/logo.png and the crates are
# a workspace; .dockerignore keeps the large things out.
COPY . .

# Cache mounts rather than the usual dance of building dummy crates first. That
# trick caches a layer, but it also leaves stub artefacts in target/ that the
# real sources are then compared against — and cargo stamps by mtime, so the
# stubs win and the binary ships empty. This is both simpler and harder to get
# subtly wrong.
#
# The mounts are not part of the image, so the binary has to be copied out of
# target/ inside the same step that builds it.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    set -eux; \
    cargo build --release --bin cshop; \
    cp target/release/cshop /cshop; \
    strip /cshop

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# libvulkan1 is the loader, mesa-vulkan-drivers carries lavapipe. The fonts are
# not optional: type is drawn from the families actually installed, and a slim
# image has none, so `text` would fail on a server and nowhere else.
# fonts-urw-base35 is what provides the families the documentation uses.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        libvulkan1 \
        mesa-vulkan-drivers \
        fonts-dejavu-core \
        fonts-liberation2 \
        fonts-urw-base35 \
        curl \
        tini; \
    rm -rf /var/lib/apt/lists/*

COPY --from=build /cshop /usr/local/bin/cshop
COPY styles /usr/local/share/cshop/styles

# Styles are looked for beside the binary, among other places.
RUN ln -s /usr/local/share/cshop/styles /usr/local/bin/styles

# Vulkan explicitly, rather than letting wgpu fall back to a GL software path
# that would be slower and is not what the tests cover.
ENV WGPU_BACKEND=vulkan \
    RUST_LOG=cshop=info,warn \
    CSHOP_ADDR=0.0.0.0:7333 \
    CSHOP_WORKSPACE=/workspace \
    HOME=/home/cshop \
    XDG_RUNTIME_DIR=/tmp
# Asks the NVIDIA container runtime for the Vulkan ICD when a GPU is passed
# through. Ignored entirely when one is not.
ENV NVIDIA_DRIVER_CAPABILITIES=graphics,compute,utility \
    NVIDIA_VISIBLE_DEVICES=all

COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Nothing here needs root, and the thing being served writes files.
#
# uid 1000 rather than a system uid, because the workspace is normally a bind
# mount from the host and 1000 is the first ordinary user on almost every Linux
# desktop and server. Getting this wrong makes every `export` fail with a
# permission error and nothing else, which is a miserable thing to debug — so
# the entrypoint also checks it can write there and says so at startup.
# Override with --build-arg UID=… , or at run time with --user.
ARG UID=1000
ARG GID=1000
RUN set -eux; \
    groupadd --gid "$GID" cshop; \
    useradd --uid "$UID" --gid "$GID" --create-home --home-dir /home/cshop \
            --shell /usr/sbin/nologin cshop; \
    mkdir -p /workspace; \
    chown cshop:cshop /workspace
USER cshop
# So that a one-off `docker run … --run 'open photo.jpg'` resolves relative
# paths against the workspace, which is the only directory that means anything
# here. Serving does not depend on this — the sandbox roots paths itself — but
# without it the command-line form silently looks in / and finds nothing.
WORKDIR /workspace
# No VOLUME declaration. The workspace is always mounted explicitly — by
# compose, or by -v — and declaring it here would only make Docker create an
# anonymous volume for every run that forgot to, which then lingers.
EXPOSE 7333

# Prove the image can actually render before it is ever tagged. Without this a
# missing driver or font surfaces as a failed request in production rather than
# as a failed build here.
RUN set -eux; \
    printf '%s\n' \
        'new 64 48 background=#336699' \
        'text 6 30 "ok" size=22 family="Nimbus Sans" color=#ffffff' \
        'export /tmp/smoke.png' > /tmp/smoke.cshop; \
    cshop --script /tmp/smoke.cshop | tee /tmp/smoke.log; \
    grep -q '0 failed' /tmp/smoke.log; \
    grep -q 'Type' /tmp/smoke.log; \
    test -s /tmp/smoke.png; \
    rm -f /tmp/smoke.png /tmp/smoke.log /tmp/smoke.cshop

# The port comes off CSHOP_ADDR rather than being repeated, so that changing
# the bind address does not quietly leave the check pointing at the old one.
# /health needs no token, which is why a check can use it at all.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${CSHOP_ADDR##*:}/health" >/dev/null || exit 1

# tini reaps the connection threads' children and passes signals through, so
# `docker stop` is a stop rather than a ten-second wait and a kill.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/docker-entrypoint.sh"]
