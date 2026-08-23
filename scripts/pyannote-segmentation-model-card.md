---
license: mit
library_name: onnx
tags:
  - speaker-diarization
  - speaker-segmentation
  - voice-activity-detection
  - overlapped-speech-detection
  - onnx
  - tract
base_model: pyannote/segmentation-3.0
---

# pyannote segmentation 3.0 — static-shape ONNX for pure-Rust inference

`pyannote/segmentation-3.0` re-exported so that it can be loaded by
[`tract`](https://github.com/sonos/tract), the pure-Rust ONNX runtime. Functionally
identical to the upstream export; only the graph's shape handling differs.

Used by [CallMind](https://github.com/shareed2k/callmind) to measure how many
speakers a recording holds instead of assuming two.

## Why a re-export was needed

The published ONNX export cannot be loaded by `tract`. It contains an `If` node
guarding a branch on the input's shape, plus symbolic dimension arithmetic that
`tract` declines to prove equal (`-24 + T/10` against `-25 + (T+9)/10`). Both
disappear once the input is fixed to a single 10-second chunk: the branch
condition becomes a constant and ordinary constant folding removes the node.

Verified against the source export:

| check | result |
| :--- | :--- |
| output change from folding | **exactly 0.0** |
| `tract` against `onnxruntime` | 6.7e-4 max, 3.0e-4 mean (f32 accumulation) |
| per-frame decision agreement | **589 / 589 frames** |

Operators after folding are all standard `ai.onnx`: `InstanceNormalization`,
`Conv`, `MaxPool`, `LeakyRelu`, `Transpose`, `LSTM`, `Reshape`, `Gemm`,
`LogSoftmax`, `Abs`.

The int8 variant is deliberately **not** provided: it folds to a graph containing
`DynamicQuantizeLSTM` from the `com.microsoft` domain, which `tract` does not
implement.

## Interface

| | |
| :--- | :--- |
| input `x` | `[1, 1, 160000]` — one 10-second chunk, 16 kHz mono, float32 |
| output `y` | `[1, 589, 7]` — per-frame log-probabilities over a speaker powerset |

The seven classes are `∅, {1}, {2}, {3}, {1,2}, {1,3}, {2,3}` — up to three
speakers with the two-at-once combinations, which is how overlapping speech is
represented. Frame duration is `10000 / 589 ≈ 16.98 ms`.

Longer audio is processed one chunk at a time; the final chunk is zero-padded.

## Measured behaviour

Against labelled recordings — four single-speaker recordings confirmed by their
owner, and two-party phone calls spanning 1 s to 13 min — taking the **median**
number of distinct speakers seen per chunk:

| statistic | single speaker | two-party |
| :--- | ---: | ---: |
| maximum per chunk | 4/4 | 20/30 |
| **median per chunk** | **4/4** | **23/24** |

The two figures come from different samples: the maximum was scored on thirty
calls before the median was adopted, the median on twenty-four spanning the full
length range. The single miss is a 9.9-second call reported as three speakers.

The maximum is the wrong statistic despite looking natural: it is the maximum of a
noisy quantity, so it grows with recording length and long calls reliably report
one speaker too many.

## Reproducing this file

```bash
python3 -m venv .venv && .venv/bin/pip install onnx onnxruntime numpy
.venv/bin/python scripts/export_pyannote_segmentation.py \
    --out models/diarization/segmentation.onnx
```

The script re-downloads the source, applies the transformation and refuses to
write a result whose output drifted, whose `If` node survived, or which contains
non-standard operator domains.

## License and attribution

MIT, inherited unchanged from upstream.

- Model and weights: `pyannote/segmentation-3.0`, **Copyright (c) 2022 CNRS**.
- Source ONNX export: [`csukuangfj/sherpa-onnx-pyannote-segmentation-3-0`](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0).
- An ungated mirror of the PyTorch weights is published as
  [`ivrit-ai/pyannote-segmentation-3.0`](https://huggingface.co/ivrit-ai/pyannote-segmentation-3.0).

If you use pyannote in research, cite the upstream work:

```bibtex
@inproceedings{Plaquet23,
  author={Alexis Plaquet and Hervé Bredin},
  title={{Powerset multi-class cross entropy loss for neural speaker diarization}},
  year=2023,
  booktitle={Proc. INTERSPEECH 2023},
}
@inproceedings{Bredin23,
  author={Hervé Bredin},
  title={{pyannote.audio 2.1 speaker diarization pipeline: principle, benchmark, and recipe}},
  year=2023,
  booktitle={Proc. INTERSPEECH 2023},
}
```
