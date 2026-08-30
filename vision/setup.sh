#!/usr/bin/env bash
# Install the vision pack: a Python environment and two ONNX models.
#
# Kept out of the editor's own build on purpose. C-Shop has no dependencies
# worth the name and builds offline; a neural network runtime is neither, so
# this lives beside it as something you opt into. Nothing here is needed to
# open, edit or save an image — only `detect` and `segment` ask for it.

set -euo pipefail

home="${CSHOP_VISION_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/cshop/vision}"
models="$home/models"
venv="$home/venv"

echo "Installing the C-Shop vision pack into $home"
mkdir -p "$models"

if [ ! -x "$venv/bin/python" ]; then
    echo "  creating a Python environment"
    python3 -m venv "$venv"
fi
echo "  installing onnxruntime, numpy, pillow"
"$venv/bin/pip" install --quiet --upgrade --no-input onnxruntime numpy pillow

# Two models. YOLOv8n finds things and says where they are; MobileSAM turns a
# point or a box into a mask. Both are ONNX so that the runtime is the only
# dependency — no PyTorch, no CUDA, no compiler.
fetch() {
    local name="$1" url="$2"
    if [ -s "$models/$name" ]; then
        echo "  $name already here"
        return
    fi
    echo "  fetching $name"
    curl -sSL --fail -o "$models/$name.part" "$url"
    mv "$models/$name.part" "$models/$name"
}

fetch yolov8n.onnx \
    "https://huggingface.co/webml/yolov8n/resolve/main/onnx/yolov8n.onnx"

if [ ! -s "$models/mobile_sam.encoder.onnx" ] || [ ! -s "$models/sam.decoder.onnx" ]; then
    echo "  fetching MobileSAM"
    curl -sSL --fail -o "$models/sam.zip" \
        "https://huggingface.co/vietanhdev/segment-anything-onnx-models/resolve/main/mobile_sam_20230629.zip"
    "$venv/bin/python" - "$models" <<'PY'
import sys, zipfile, os, shutil
models = sys.argv[1]
with zipfile.ZipFile(os.path.join(models, "sam.zip")) as z:
    z.extractall(models)
# The decoder is shared across every SAM size, so it is stored under a name
# that says which one it came from. Give it the name the runner looks for.
for name in os.listdir(models):
    if name.endswith(".decoder.onnx") and name != "sam.decoder.onnx":
        shutil.move(os.path.join(models, name), os.path.join(models, "sam.decoder.onnx"))
PY
    # config.yaml is kept: it carries the frame the encoder expects, which the
    # runner reads rather than assuming.
    rm -f "$models/sam.zip"
fi

echo
echo "Done. $(du -sh "$home" | cut -f1) in $home"
"$venv/bin/python" "$(dirname "$0")/cshop-vision.py" check
