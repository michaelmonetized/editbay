# Native inference feasibility

Measured locally on 2026-10-03, Linux ARM64 / Apple M1 Pro. These are native
feasibility tools. No model weights, client pictures or recurrent-state payloads
are published in this repository. The inference candidates do not modify projects.

## Implemented path

`editbay-ai` verifies the exact bytes loaded by native ONNX Runtime. The runtime
library is selected explicitly before model loading. Inputs and outputs are finite,
shape checked and bounded. RVM runs original RGB pictures in source order, retains
four recurrent tensors, and produces soft alpha plus foreground RGB. SAM 2.1 runs
all five video graphs: vision, mask decoder, memory encoder, memory attention and
object-pointer temporal position. It retains conditioning memory plus at most
15 recent frames. Point prompts and resulting logits map to original coordinates.
Segmentation logits are separate from opacity alpha.

Snapshots bind source digest, exact model revision, dimensions, configuration,
next source frame and memory. A cut, changed source or changed configuration needs
a new shot. Invalid frame order, forged recurrent geometry, cancelled operations
and failed graphs cannot advance state. A 5 ms watcher delivers cancellation to
ONNX Runtime's native termination flag, then the caller rejects any late result.
The library remains a synchronous worker primitive; UI scheduling and persistent
result assets belong to R2/R4.

The optional `editbay-reference` adapter runs the official RVM TorchScript model
through libtorch's C++ API with Rust-owned inputs/outputs. Python is never installed
or executed. Optional Tract reference runs use independent Rust graph kernels;
they share the SAM preprocessing and memory-bank orchestration. That reference
path checks cancellation between graphs, and is not the artist worker.

The encoder also now writes through an owned file descriptor and custom FFmpeg
IO. It rejects existing nonempty files and symlinks. Replacing the pathname after
open cannot redirect output into a replacement asset; regression coverage checks
that replacement bytes survive and the owned output decodes correctly.

## Measurements

The client source is the existing Mack's Shack exterior shot, local
`Downloads/iCloud Photos/IMG_0001.MP4`, 1280×720 at 24 fps. Its digest is
`498ec6c42b6b440886eb761dc4775c994f7578021890ceae3f370ffc6fb3e021`.
Only derived numerical receipts are published. Client media ownership and release
permissions for a complete reference-job inventory remain EB-006 work.

| Probe | Actual result | Limit |
| --- | --- | --- |
| RVM native pass, 6 source frames | p95 66.94 ms; exact resume after frame 3 | CPU, two threads; ratio 0.25; no annotated matte quality |
| RVM active cancellation | 22.85 ms total, including a 20 ms request delay; state preserved | Model load is checked before/after, not interrupted inside session construction |
| RVM independent official TorchScript, 8 frames | Worst alpha MAE 0.000000004745; maximum alpha error 0.000006065; foreground MAE 0.0000003633 | Identical decoded inputs; native kernel agreement, not human matte acceptance |
| SAM native motion fixture, 6 frames | p95 6431.60 ms; exact resume after frame 3 | Single object; first-frame point prompts; no later anchors yet |
| SAM client tree/trunk prompt, 3 frames | p95 6333.37 ms; exact resume after frame 1 | No human-labelled client mask truth |
| SAM client active cancellation | 85.03 ms total, including a 20 ms request delay; state preserved | CPU/ARM64 only |
| SAM independent Tract pass, 6 motion frames | Mask IoU 1.0 on every frame; worst logit MAE 0.000185916; maximum error 0.000944138 | Shared orchestration; upstream PyTorch video orchestration parity remains separate |
| SAM analytic original-coordinate motion masks | Worst IoU 0.993624, above 0.99 gate | Simple rectangle, not hair/translucency/occlusion/client acceptance |
| SAM independent Tract pass, 3 client frames | Mask IoU 1.0 on every frame; worst logit MAE 0.000184357; maximum error 0.001922608 | Numerical agreement, not artist acceptance |

The largest measured SAM process high-water mark was 5501952 KiB (about 5.25 GiB).
The comparison/resume probes deliberately instantiate separate models and retain
original pictures and outputs. Steady-state pipeline memory, long-shot state growth
and production resource admission remain unqualified. CPU SAM is suitable for an
offline feasibility run; no interactive frame-rate claim is made.

## Reproduce

Build on the local machine. The default application does not link libtorch or Tract.
Use the explicit pinned native runtime and model pack paths in the lab:

```sh
cargo run --release --locked -p editbay-lab -- rvm SOURCE RVM_ONNX ORT_LIBRARY 6
cargo run --release --locked -p editbay-lab -- sam2 SOURCE SAM2_PACK ORT_LIBRARY 3 X Y
cargo run --release --locked -p editbay-lab -- sam2-fixture NEW_MKV

EDITBAY_LIBTORCH=/path/to/verified/torch cargo build --release --locked -p editbay-lab --all-features
LD_LIBRARY_PATH=/path/to/verified/torch/lib target/release/editbay-lab rvm-parity SOURCE RVM_ONNX RVM_TORCHSCRIPT ORT_LIBRARY 8
LD_LIBRARY_PATH=/path/to/verified/torch/lib target/release/editbay-lab sam2-parity SOURCE SAM2_PACK ORT_LIBRARY 3 X Y
LD_LIBRARY_PATH=/path/to/verified/torch/lib target/release/editbay-lab sam2-motion-parity SAM2_PACK ORT_LIBRARY
```

For Tract alone, use `--features tract-reference`; libtorch is unnecessary.
The generated motion fixture is six lossless pictures of a 100×120 rectangle
moving 10 original pixels per frame on a 512×288 background. The reference command
also checks the analytic original-coordinate mask at each frame against IoU 0.99.

## Artifact and runtime terms

See `models.json` for immutable URLs, revisions, digests and license evidence.
The RVM official artifact/code is evaluated under GPL-3.0 terms. That is not a
noncommercial ban. Distribution of an EditBay model pack must meet applicable
source, license and notice obligations before enablement; no weight is bundled here.
SAM 2 code is Apache-2.0. The pinned graph author's model card declares Apache-2.0
and the official SAM 2.1 Tiny base model; its export uses Transformers 5.11.
The export source is not published in that pack. Do not represent this as a
reproducible upstream conversion or a cleared finished artist workflow.

ONNX Runtime 1.28.0 has MIT terms. This probe reads the independently installed
ARM64 runtime without changing Omadesign. EditBay must package its own approved
native runtime; it will not depend on an Omadesign installation path. The libtorch
reference uses native libraries/headers extracted from the official 2.10.0 CPU
ARM64 wheel, verified against the publisher's SHA-256. The wheel's license/notice
files are retained locally; Python modules were neither extracted nor executed.
Tract 0.23.8 is pinned in Cargo.lock under MIT or Apache-2.0 terms.
The loaded runtime reports commit `da9b5e364c`, a release build and its fp8 KV-cache
flag; see `runtime.json`. Native/reference timing and resume receipts were gathered
during the prototype implementation. The final client-reference receipts and
runtime inspection use lab binary SHA-256
`2c8f3736525acb58273fbafc56cece2c7d088eddd221b7ce4c86d4fbd0368bd7`;
current source/lock/binary hashes are recorded in `provenance.sha256`.

## Remaining production gates

Qualify source-resolution edges, translucent material, hair/hands, fast movement,
cuts, occlusion/re-entry and multiple subjects against annotated footage. Compare
SAM's upstream video orchestration, support correction anchors and shot resets,
measure long-video memory and artist time, and validate the claimed GPU/backend
matrix. Build verified optional packs, cancellation/retry, runtime packaging,
source-time result assets, stale-revision rejection, correction/undo, refinement,
composites, portable handoff and actual editor/export use. These are R2/R4/R11
work; numerical inference agreement alone does not complete them.
