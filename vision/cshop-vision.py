#!/usr/bin/env python3
"""The vision pack: object detection and segmentation, as a sidecar.

C-Shop runs this as a subprocess and reads JSON from its standard output.
That boundary is deliberate. Neural network runtimes are large, platform
specific and change often; the editor is a single binary with almost no
dependencies that builds offline. Keeping the two apart means installing the
models cannot break the editor, and not installing them costs nothing.

Two models:

* **YOLOv8n** says what is in a picture and where — a class, a confidence and
  a box. It sees eighty kinds of thing and nothing else, which is the honest
  limit of it.
* **MobileSAM** turns a point or a box into a mask. It does not know what
  anything *is*; it separates whatever the prompt points at from its
  surroundings. That is why the two are better together than either alone:
  YOLO finds the dog, SAM cuts it out.
* **SCUNet** removes noise. A Swin-Conv-UNet: transformer blocks inside a
  UNet, which is what makes it quick enough to be worth waiting for — see
  `denoise` below for the arithmetic.

Subcommands print one JSON object to stdout. Errors print JSON too, with
`"ok": false` and a message, so the caller never has to parse a traceback.
"""

import argparse
import hashlib
import json
import os
import sys

HOME = os.environ.get("CSHOP_VISION_HOME") or os.path.join(
    os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), "cshop", "vision"
)
MODELS = os.path.join(HOME, "models")
CACHE = os.path.join(HOME, "embeddings")

# What YOLOv8 was trained on, in the order its output channels come in.
COCO = (
    "person bicycle car motorcycle airplane bus train truck boat traffic-light "
    "fire-hydrant stop-sign parking-meter bench bird cat dog horse sheep cow "
    "elephant bear zebra giraffe backpack umbrella handbag tie suitcase frisbee "
    "skis snowboard sports-ball kite baseball-bat baseball-glove skateboard "
    "surfboard tennis-racket bottle wine-glass cup fork knife spoon bowl banana "
    "apple sandwich orange broccoli carrot hot-dog pizza donut cake chair couch "
    "potted-plant bed dining-table toilet tv laptop mouse remote keyboard "
    "cell-phone microwave oven toaster sink refrigerator book clock vase "
    "scissors teddy-bear hair-drier toothbrush"
).split()

# The side the detector's input is squared off to.
YOLO_SIZE = 640

# The frame the segmenter's encoder was exported for.
#
# Not a square, and not "the long side is 1024" — the model ships a config
# saying `max_width: 1024, max_height: 682`, and it means it. Feeding a
# portrait picture scaled to a 1024 long side instead put every mask 1.5 times
# too far down the frame, 1.5 being exactly 1024/682. The shape was right and
# the placement was wrong, which is the kind of error that looks plausible on a
# subject in the middle of the picture and is obvious on one that is not.
SAM_MAX_W, SAM_MAX_H = 1024, 682


def sam_frame():
    """The encoder's frame, from the model's own config where it is present."""
    path = os.path.join(MODELS, "config.yaml")
    w, h = SAM_MAX_W, SAM_MAX_H
    try:
        with open(path) as f:
            for line in f:
                key, _, value = line.partition(":")
                if key.strip() == "max_width":
                    w = int(value.strip())
                elif key.strip() == "max_height":
                    h = int(value.strip())
    except (OSError, ValueError):
        pass
    return w, h


# The denoiser's constraints, which are not negotiable and not documented
# anywhere except in the shapes it refuses.
#
# Its Swin windows are 8 pixels and it downsamples three times, so every side
# it is given must be a multiple of 8 x 2^3 = 64. Anything else fails inside a
# reshape with a message about tensors, which is a poor way to find out.
DENOISE_MULTIPLE = 64

# How much picture goes through at a time, and how much neighbouring tiles
# share. Overlap is what stops the seams: each tile is weighted by a taper that
# falls to nothing at its edge, so where two tiles meet they cross-fade instead
# of butting up against each other with slightly different ideas about the
# noise. Thirty-two pixels is enough for that and cheap: the waste is the
# overlap area, which at 256 with 32 is about a quarter.
DENOISE_TILE = 256
DENOISE_OVERLAP = 32


def fail(message, **extra):
    print(json.dumps({"ok": False, "error": message, **extra}))
    sys.exit(2)


def need(path, what):
    if not os.path.exists(path):
        fail(
            f"the vision pack is not installed: {what} is missing",
            missing=path,
            hint="run vision/setup.sh",
        )
    return path


def session(name):
    import onnxruntime as ort

    path = need(os.path.join(MODELS, name), name)
    options = ort.SessionOptions()
    # One model at a time, and the caller is already a separate process, so
    # there is nothing to gain from spawning more threads than cores.
    options.log_severity_level = 3
    return ort.InferenceSession(path, options, providers=["CPUExecutionProvider"])


# ---------------------------------------------------------------------------
# Detection
# ---------------------------------------------------------------------------


def letterbox(image, size):
    """Fit the image into a square without distorting it.

    Returns the canvas and the scale, since every coordinate the model gives
    back has to be divided by that scale to mean anything about the original.
    """
    from PIL import Image

    w, h = image.size
    scale = min(size / w, size / h)
    resized = image.resize((max(1, round(w * scale)), max(1, round(h * scale))), Image.BILINEAR)
    canvas = Image.new("RGB", (size, size), (114, 114, 114))
    canvas.paste(resized, (0, 0))
    return canvas, scale


def iou(a, b):
    x0, y0 = max(a[0], b[0]), max(a[1], b[1])
    x1, y1 = min(a[2], b[2]), min(a[3], b[3])
    inter = max(0.0, x1 - x0) * max(0.0, y1 - y0)
    union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / union if union > 0 else 0.0


def detect(image_path, conf, want_classes, iou_threshold=0.45):
    import numpy as np
    from PIL import Image

    image = Image.open(image_path).convert("RGB")
    width, height = image.size
    canvas, scale = letterbox(image, YOLO_SIZE)
    x = np.asarray(canvas, np.float32).transpose(2, 0, 1)[None] / 255.0

    out = session("yolov8n.onnx").run(None, {"images": x})[0][0]

    # Rows are 4 box values then one score per class, columns are candidate
    # positions. Scored first, then thinned, because non-maximum suppression
    # over every one of eight thousand candidates would be wasted work.
    scores = out[4:, :]
    best_class = scores.argmax(axis=0)
    best_score = scores.max(axis=0)
    keep = np.nonzero(best_score >= conf)[0]

    found = []
    for i in keep:
        cx, cy, w, h = out[0:4, i]
        box = [
            float(cx - w / 2) / scale,
            float(cy - h / 2) / scale,
            float(cx + w / 2) / scale,
            float(cy + h / 2) / scale,
        ]
        found.append((float(best_score[i]), int(best_class[i]), box))

    found.sort(key=lambda d: -d[0])
    kept = []
    for score, cls, box in found:
        # Suppression is per class: a dog overlapping a bench is two things,
        # the same dog found twice is one.
        if any(c == cls and iou(box, b) >= iou_threshold for _, c, b in kept):
            continue
        kept.append((score, cls, box))

    results = []
    for score, cls, box in kept:
        name = COCO[cls] if cls < len(COCO) else str(cls)
        if want_classes and name not in want_classes:
            continue
        box = [
            max(0.0, min(box[0], width)),
            max(0.0, min(box[1], height)),
            max(0.0, min(box[2], width)),
            max(0.0, min(box[3], height)),
        ]
        results.append(
            {
                "class": name,
                "score": round(score, 4),
                "box": [round(v, 1) for v in box],
                "width": round(box[2] - box[0], 1),
                "height": round(box[3] - box[1], 1),
            }
        )
    return results, (width, height)


# ---------------------------------------------------------------------------
# Segmentation
# ---------------------------------------------------------------------------


def embedding_for(image_path):
    """The encoder's view of the image, computed once and kept.

    Encoding is nearly all of the cost of segmenting — the decoder that turns
    a prompt into a mask is milliseconds. Caching it by content means clicking
    a second point on the same picture is immediate, which is the difference
    between an interactive tool and a batch one.
    """
    import numpy as np
    from PIL import Image

    with open(image_path, "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()[:32]
    os.makedirs(CACHE, exist_ok=True)
    cached = os.path.join(CACHE, digest + ".npy")

    image = Image.open(image_path).convert("RGB")
    width, height = image.size
    max_w, max_h = sam_frame()
    # Fitted into the frame, so neither side overruns it.
    scale = min(max_w / width, max_h / height)

    if os.path.exists(cached):
        return np.load(cached), (width, height), scale, True

    resized = image.resize((max(1, round(width * scale)), max(1, round(height * scale))), Image.BILINEAR)
    embedding = session("mobile_sam.encoder.onnx").run(
        None, {"input_image": np.asarray(resized, np.float32)}
    )[0]
    np.save(cached, embedding)
    return embedding, (width, height), scale, False


def segment(image_path, box, points, negatives):
    import numpy as np

    embedding, (width, height), scale, cached = embedding_for(image_path)

    coords, labels = [], []
    if box is not None:
        # A box is given as its two corners, labelled 2 and 3.
        coords += [[box[0] * scale, box[1] * scale], [box[2] * scale, box[3] * scale]]
        labels += [2.0, 3.0]
    for x, y in points:
        coords.append([x * scale, y * scale])
        labels.append(1.0)
    for x, y in negatives:
        coords.append([x * scale, y * scale])
        labels.append(0.0)
    if not coords:
        fail("segmenting needs a box or at least one point")

    # `orig_im_size` is asked for as the model's frame rather than the picture.
    #
    # This decoder stretches its result across whatever size it is given
    # without first cutting off the padding the encoder added. Asking for the
    # frame and cropping it here is the same postprocessing the reference
    # implementation does, and it is what puts the mask where the object is.
    masks, scores, _ = session("sam.decoder.onnx").run(
        None,
        {
            "image_embeddings": embedding,
            "point_coords": np.array([coords], np.float32),
            "point_labels": np.array([labels], np.float32),
            "mask_input": np.zeros((1, 1, 256, 256), np.float32),
            "has_mask_input": np.zeros(1, np.float32),
            # Asked for in the model's own frame, then cropped back to the
            # picture below. Asking for the picture's size instead leaves the
            # padding stretched across it.
            "orig_im_size": np.array(list(reversed(sam_frame())), np.float32),
        },
    )
    # The decoder offers several readings of the same prompt — roughly the
    # part, the object, and the whole — and rates them. Taking the highest
    # rating alone is not enough: for a point in a large flat region it will
    # happily rate "the entire picture" best, which is never what someone
    # meant by clicking on something. Readings that cover almost everything
    # are set aside unless they are all there is.
    candidates = list(range(masks.shape[1]))
    best = max(candidates, key=lambda i: scores[0][i])
    mask = crop_and_resize(masks[0, best], width, height, scale)
    return mask, float(scores[0][best]), (width, height), cached


def crop_and_resize(mask, width, height, scale):
    """Take the picture back out of the padded frame the model works in."""
    import numpy as np
    from PIL import Image

    nw, nh = max(1, round(width * scale)), max(1, round(height * scale))
    cropped = mask[:nh, :nw]
    # Resampled as floats, so the ramp either side of the boundary survives to
    # become an antialiased edge rather than a staircase.
    return np.asarray(
        Image.fromarray(cropped.astype(np.float32), "F").resize((width, height), Image.BILINEAR),
        np.float32,
    )


def write_mask(mask, path):
    from PIL import Image

    # The raw output is a signed distance from the boundary, positive inside.
    # Rather than threshold it flat, the value either side of zero is mapped
    # across one pixel, which gives the edge an antialiased ramp instead of a
    # staircase — the same trick the shape rasteriser uses.
    import numpy as np

    coverage = np.clip(mask * 2.0 + 0.5, 0.0, 1.0)
    Image.fromarray((coverage * 255).astype(np.uint8), "L").save(path)
    return float((mask > 0).mean())


# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Denoising
# ---------------------------------------------------------------------------


def taper(size, overlap):
    """A weight that is flat in the middle and falls to nothing at both ends.

    Smoothstep rather than linear, so the weight's *slope* matches at the seam
    as well as its value. A linear ramp leaves a faint crease visible on a
    smooth gradient, which is exactly the kind of picture someone is denoising.
    """
    import numpy as np

    w = np.ones(size, np.float32)
    if overlap > 0:
        t = (np.arange(overlap, dtype=np.float32) + 0.5) / overlap
        ramp = t * t * (3.0 - 2.0 * t)
        w[:overlap] = ramp
        w[-overlap:] = ramp[::-1]
    return w


def tile_starts(extent, tile, stride):
    """Where each tile begins along one axis.

    The last tile is pushed flush against the far edge rather than hanging off
    it. That overlaps its neighbour by more than the others do, which the
    weighting handles for nothing, and it means no padding beyond what the
    model's multiple-of-64 rule already demands.
    """
    if extent <= tile:
        return [0]
    starts = list(range(0, extent - tile, stride))
    if not starts or starts[-1] != extent - tile:
        starts.append(extent - tile)
    return starts


def denoise(image_path, out_path, strength):
    """Remove noise, a tile at a time, reporting progress as it goes.

    Alpha is carried through untouched. Noise is a property of the colour a
    sensor recorded; coverage is not something a camera measured, and running
    it through a denoiser would soften the edge of a cut-out for no reason.
    """
    import numpy as np
    from PIL import Image

    sess = session("scunet_color_real_psnr.onnx")
    name = sess.get_inputs()[0].name

    image = Image.open(image_path)
    has_alpha = image.mode in ("RGBA", "LA") or "transparency" in image.info
    rgb = np.asarray(image.convert("RGB"), np.float32) / 255.0
    alpha = np.asarray(image.convert("RGBA"), np.uint8)[..., 3] if has_alpha else None
    height, width, _ = rgb.shape

    # Up to a multiple of 64, by reflection, so the edges of the picture see
    # picture rather than black.
    pad_h = (-height) % DENOISE_MULTIPLE
    pad_w = (-width) % DENOISE_MULTIPLE
    padded = np.pad(rgb, ((0, pad_h), (0, pad_w), (0, 0)), mode="reflect")
    ph, pw, _ = padded.shape

    tile = min(DENOISE_TILE, ph, pw)
    tile -= tile % DENOISE_MULTIPLE
    tile = max(tile, DENOISE_MULTIPLE)
    overlap = min(DENOISE_OVERLAP, tile // 4)
    stride = max(tile - overlap, DENOISE_MULTIPLE)

    ys = tile_starts(ph, tile, stride)
    xs = tile_starts(pw, tile, stride)
    weight_1d = taper(tile, overlap)
    window = np.outer(weight_1d, weight_1d)[..., None]

    total = len(ys) * len(xs)
    print(json.dumps({"tiles": total}), file=sys.stderr, flush=True)

    acc = np.zeros_like(padded)
    wsum = np.zeros((ph, pw, 1), np.float32)
    done = 0
    for y in ys:
        for x in xs:
            patch = padded[y : y + tile, x : x + tile].transpose(2, 0, 1)[None]
            out = sess.run(None, {name: patch})[0][0].transpose(1, 2, 0)
            acc[y : y + tile, x : x + tile] += out * window
            wsum[y : y + tile, x : x + tile] += window
            done += 1
            # One line per tile, so the caller's progress bar moves at the rate
            # the work actually happens rather than at a guessed one.
            print(json.dumps({"tile": done, "tiles": total}), file=sys.stderr, flush=True)

    cleaned = np.clip(acc / np.maximum(wsum, 1e-6), 0.0, 1.0)[:height, :width]

    # Strength mixes the result back over the original. The model was trained
    # on one kind of noise and a photograph has its own; being able to take
    # half of what it decided is the difference between a tool and a verdict.
    s = float(min(max(strength, 0.0), 1.0))
    if s < 1.0:
        cleaned = cleaned * s + rgb * (1.0 - s)

    out = (cleaned * 255.0 + 0.5).astype(np.uint8)
    if alpha is not None:
        out = np.dstack([out, alpha])
    Image.fromarray(out, "RGBA" if alpha is not None else "RGB").save(out_path)

    # How much it actually changed, so a caller can tell "there was no noise"
    # from "nothing happened".
    moved = float(np.mean(np.abs(cleaned - rgb)) * 255.0)
    return {
        "ok": True,
        "path": out_path,
        "width": width,
        "height": height,
        "tiles": total,
        "strength": s,
        "moved": round(moved, 3),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("check", help="report whether the pack is installed")

    d = sub.add_parser("detect", help="find objects")
    d.add_argument("--image", required=True)
    d.add_argument("--conf", type=float, default=0.25)
    d.add_argument("--classes", default="", help="comma-separated, to keep only these")

    n = sub.add_parser("denoise", help="remove noise")
    n.add_argument("image")
    n.add_argument("--out", required=True)
    n.add_argument("--strength", type=float, default=1.0)

    s = sub.add_parser("segment", help="cut something out")
    s.add_argument("--image", required=True)
    s.add_argument("--out", required=True, help="where to write the mask, as a grey PNG")
    s.add_argument("--box", help="x0,y0,x1,y1")
    s.add_argument("--point", action="append", default=[], help="x,y — repeatable")
    s.add_argument("--not-point", action="append", default=[], help="x,y to exclude")
    s.add_argument("--class", dest="klass", help="detect this and segment the best one")
    s.add_argument("--conf", type=float, default=0.25)

    args = parser.parse_args()

    if args.command == "check":
        missing = [
            n
            for n in (
                "yolov8n.onnx",
                "mobile_sam.encoder.onnx",
                "sam.decoder.onnx",
                "scunet_color_real_psnr.onnx",
                # The weights live beside the graph and are referred to by
                # name; without them the model loads and then fails, which is
                # a worse way to find out than being told here.
                "scunet_color_real_psnr.onnx.data",
            )
            if not os.path.exists(os.path.join(MODELS, n))
        ]
        try:
            import onnxruntime  # noqa: F401
            import numpy  # noqa: F401
            from PIL import Image  # noqa: F401
        except Exception as e:  # pragma: no cover - only when half-installed
            print(json.dumps({"ok": False, "error": f"python packages missing: {e}"}))
            sys.exit(2)
        print(json.dumps({"ok": not missing, "home": HOME, "missing": missing}))
        sys.exit(0 if not missing else 2)

    if not os.path.exists(args.image):
        fail(f"no such image: {args.image}")

    def pairs(values):
        out = []
        for v in values:
            try:
                x, y = (float(p) for p in v.split(","))
            except ValueError:
                fail(f"{v!r} is not an x,y point")
            out.append((x, y))
        return out

    if args.command == "denoise":
        print(json.dumps(denoise(args.image, args.out, args.strength)))
        return

    if args.command == "detect":
        wanted = {c.strip() for c in args.classes.split(",") if c.strip()}
        found, size = detect(args.image, args.conf, wanted)
        print(json.dumps({"ok": True, "width": size[0], "height": size[1], "objects": found}))
        return

    box = None
    detected = None
    if args.box:
        try:
            box = [float(v) for v in args.box.split(",")]
            assert len(box) == 4
        except (ValueError, AssertionError):
            fail(f"{args.box!r} is not x0,y0,x1,y1")
    elif args.klass:
        found, _ = detect(args.image, args.conf, {args.klass})
        if not found:
            fail(f"nothing recognised as {args.klass!r} in this picture")
        detected = found[0]
        box = detected["box"]

    mask, score, size, cached = segment(args.image, box, pairs(args.point), pairs(args.not_point))
    covered = write_mask(mask, args.out)
    print(
        json.dumps(
            {
                "ok": True,
                "mask": args.out,
                "width": size[0],
                "height": size[1],
                "confidence": round(score, 4),
                "coverage": round(covered, 4),
                "cached_embedding": cached,
                "detected": detected,
            }
        )
    )


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001 - the caller reads JSON, not tracebacks
        fail(f"{type(e).__name__}: {e}")
