#!/bin/sh
# Turn environment into flags, and refuse to be careless about the token.
set -eu

# Anything passed after the image name is taken as the whole command line, so
# the image stays usable for one-off work and not only for serving:
#
#   docker run --rm -v "$PWD:/workspace" cshop --run 'new 10 10
#   export out.png'
if [ "$#" -gt 0 ]; then
    exec cshop "$@"
fi

ADDR="${CSHOP_ADDR:-0.0.0.0:7333}"
WORKSPACE="${CSHOP_WORKSPACE:-/workspace}"

# A bind-mounted workspace belongs to whoever owns it on the host, which need
# not be the user in this image. Left alone, that surfaces as every `export`
# failing with a permission error and nothing pointing at why — so check once,
# here, and say exactly what to do about it.
if ! mkdir -p "$WORKSPACE" 2>/dev/null || ! [ -w "$WORKSPACE" ]; then
    owner="$(stat -c '%u:%g' "$WORKSPACE" 2>/dev/null || echo 'unknown')"
    echo "cshop: cannot write to $WORKSPACE" >&2
    echo >&2
    echo "  it is owned by  $owner" >&2
    echo "  this container is  $(id -u):$(id -g)" >&2
    echo >&2
    echo "  Run as the owner:   docker run --user $owner ..." >&2
    echo "  or in compose:      user: \"$owner\"" >&2
    echo "  or hand it over:    sudo chown -R $(id -u):$(id -g) <the host directory>" >&2
    exit 1
fi

# A container has to bind 0.0.0.0 to be reachable at all, which is exactly the
# case the editor refuses to serve without a token — rightly, since a script can
# read and write files in its workspace. Rather than fail, or quietly drop the
# requirement, generate one and say so loudly. Set CSHOP_TOKEN to choose it
# yourself, which is what you want as soon as anything else has to know it.
if [ -z "${CSHOP_TOKEN:-}" ]; then
    CSHOP_TOKEN="$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    echo "-----------------------------------------------------------------"
    echo " No CSHOP_TOKEN was set, so one was generated for this container:"
    echo
    echo "     $CSHOP_TOKEN"
    echo
    echo " Send it as:  Authorization: Bearer $CSHOP_TOKEN"
    echo " Set CSHOP_TOKEN to choose your own and keep it across restarts."
    echo "-----------------------------------------------------------------"
fi

set -- --serve "$ADDR" --workspace "$WORKSPACE" --token "$CSHOP_TOKEN"

# Comma-separated, because one environment variable travels through compose and
# orchestrators more easily than a repeated flag.
if [ -n "${CSHOP_ALLOW_ORIGINS:-}" ]; then
    saved_ifs=$IFS
    IFS=,
    for origin in $CSHOP_ALLOW_ORIGINS; do
        if [ -n "$origin" ]; then
            set -- "$@" --allow-origin "$origin"
        fi
    done
    IFS=$saved_ifs
fi

exec cshop "$@"
