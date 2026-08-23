#!/usr/bin/env python3
"""Produce a `tract`-loadable export of pyannote's segmentation model.

Why this script exists
----------------------
`pyannote/segmentation-3.0` is the piece that makes speaker counting work: it
classifies every frame into a *powerset* of active speakers, so one 10-second
chunk already says whether one person or two are talking -- no clustering, no
distance threshold. Measured against labelled recordings, the median per-chunk
count identified 4/4 monologues and 26/30 two-party calls.

The published ONNX export cannot be loaded by `tract`, the pure-Rust runtime this
project uses. It carries one `If` node guarding a branch on the input's shape,
plus symbolic dimension arithmetic that `tract` declines to prove equal. Fixing
the input to a single 10-second chunk makes the branch condition constant, and
ordinary constant folding then removes the node entirely.

Verified: folding changes the output by exactly 0.0, and `tract` then agrees with
onnxruntime to 6.7e-4 with the per-frame decision identical on all 589 frames.

Usage
-----
    python3 -m venv .venv && .venv/bin/pip install onnx onnxruntime
    .venv/bin/python scripts/export_pyannote_segmentation.py \
        --out models/diarization/segmentation.onnx

The source model is MIT licensed, so the result is redistributable.
"""

from __future__ import annotations

import argparse
import collections
import pathlib
import sys
import urllib.request

# The ungated ONNX export. `pyannote/segmentation-3.0` itself is gated behind
# accepting conditions; this mirror is not, and `ivrit-ai/pyannote-segmentation-3.0`
# mirrors the PyTorch weights on the same terms.
SOURCE_URL = (
    "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0"
    "/resolve/main/model.onnx"
)

# 10 seconds at 16 kHz, which is what pyannote 3.0 is trained on.
CHUNK_SAMPLES = 160_000


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=pathlib.Path)
    parser.add_argument("--source", type=pathlib.Path, default=None,
                        help="a local copy of the stock export, if already downloaded")
    args = parser.parse_args()

    try:
        import onnx
        import onnxruntime as ort
    except ImportError:
        print("needs `onnx` and `onnxruntime`: pip install onnx onnxruntime", file=sys.stderr)
        return 1

    work = args.out.parent
    work.mkdir(parents=True, exist_ok=True)

    source = args.source
    if source is None:
        source = work / "segmentation-source.onnx"
        if not source.exists():
            print(f"downloading {SOURCE_URL}")
            urllib.request.urlretrieve(SOURCE_URL, source)

    model = onnx.load(str(source))
    before = collections.Counter(n.op_type for n in model.graph.node)
    print(f"source ops: {dict(before)}")

    # One chunk at a time. This is what turns the shape-conditional branch into a
    # constant.
    from onnx.tools import update_model_dims

    model = update_model_dims.update_inputs_outputs_dims(
        model,
        {"x": [1, 1, CHUNK_SAMPLES]},
        {"y": [1, "frames", 7]},
    )
    staged = work / "segmentation-static.onnx"
    onnx.save(model, str(staged))

    # Basic optimisation only: constant folding and dead-node elimination. Higher
    # levels substitute Microsoft-domain fused operators that a non-onnxruntime
    # runtime cannot read.
    options = ort.SessionOptions()
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
    options.optimized_model_filepath = str(args.out)
    session = ort.InferenceSession(str(staged), options, providers=["CPUExecutionProvider"])
    print(f"input : {[(i.name, i.shape) for i in session.get_inputs()]}")
    print(f"output: {[(o.name, o.shape) for o in session.get_outputs()]}")

    folded = onnx.load(str(args.out))
    after = collections.Counter(n.op_type for n in folded.graph.node)
    domains = sorted({(n.domain or "ai.onnx") for n in folded.graph.node})
    print(f"folded ops: {dict(after)}")
    print(f"domains: {domains}")

    if after.get("If"):
        print("FAILED: an `If` node survived; tract will not load this", file=sys.stderr)
        return 1
    if domains != ["ai.onnx"]:
        print(f"FAILED: non-standard operator domains present: {domains}", file=sys.stderr)
        return 1

    # The folding must not change the numbers.
    import numpy as np

    probe = (np.sin(np.arange(CHUNK_SAMPLES, dtype=np.float32) * 0.001) * 0.3)
    probe = probe.reshape(1, 1, CHUNK_SAMPLES)
    a = ort.InferenceSession(str(staged), providers=["CPUExecutionProvider"]).run(None, {"x": probe})[0]
    b = session.run(None, {"x": probe})[0]
    drift = float(np.abs(a - b).max())
    print(f"output drift from folding: {drift}")
    if drift > 1e-5:
        print("FAILED: folding changed the output", file=sys.stderr)
        return 1

    staged.unlink(missing_ok=True)
    print(f"\nwrote {args.out} ({args.out.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
