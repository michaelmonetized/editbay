# Rust restart: implementation status

Validated locally on 2026-10-03, Linux ARM64, Rust/Cargo 1.98.0.
This is the project/recovery foundation plus native engine feasibility tools.
The desktop editor and complete-job release gates remain open.

## Implemented

- Cargo workspace with `editbay-core` and the `editbay` CLI, a generated
  dependency lockfile, structured errors, and no production Python dependency.
- Fresh validated document schema: stable UUIDs, revisions, sequence dimensions,
  rational frame rates, duplicate-identity checks, and explicit schema rejection.
- Project creation/load/rename/save. Writes synchronize the file and parent
  directories; existing saves publish atomically. New project/checkpoint publication
  refuses an existing destination without an overwrite race.
- Persistent writer locks and optimistic concurrency for CLI edits. A stale
  loaded document cannot replace newer work or recreate a deleted project.
- Immutable recovery checkpoints with identity/revision/path/time metadata and
  SHA-256 integrity verification. Untitled projects can be checkpointed through
  the core API. A broken latest checkpoint leaves older valid work discoverable.
- Recovery to a new independent project with an origin ID. Recovery refuses the
  original destination and existing files, including when the source was deleted.
- The active `bin/editbay` builds/runs Rust. Application source and tests are
  confined to the Cargo workspace.
- Native `editbay-media`, `editbay-render`, `editbay-audio` and `editbay-lab`
  prototype decode, explicit FP16/FP32 GPU color/alpha composition, device-clock
  sound playback, and an isolated cancellable FFV1 picture export worker.
  Export reopens and checks every decoded picture byte before separate-file
  publication; source changes and destination conflicts fail explicitly.
- Predeclared enterprise/GTM budgets in `RELEASE_CRITERIA.md`; actual ARM64
  measurements, hashes and limits in `evidence/r0-engine/`.

## Validation

| Check | Result |
| --- | --- |
| `cargo test --workspace --locked` | 26 passed; 0 failed or ignored |
| Core regression suite | 16 tests: identity/rational time, validation, round trips, permission preservation, concurrent/stale writers, corruption, source loss and overwrite refusal |
| CLI binary integration suite | 3 tests: complete recovery flow, command failures and real process interruption during save; paths contain spaces |
| Native codec/GPU/worker/clock tests | 7 tests: lossless pictures and rational time; delayed video/audio drain; invalid codecs; actual FP16/FP32 GPU parity; export/cancel cleanup; device-clock interpolation |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `./bin/editbay --version` | `editbay 0.1.0 (Rust project foundation)` |

The tests exercise actual local files, CLI processes, native codec libraries and
the M1 Pro Vulkan GPU. A real save process was killed and restarted successfully.
The actual ALSA callback route and short playback scheduling were measured.
Power loss, native-window recovery, physical audibility/latency, other advertised
hardware and live services remain unqualified.

## Still to implement

Background autosave scheduling/ownership, checkpoint pruning, welcome/catalog UI,
native desktop shell, timeline/undo/composition schema, production ingest/playback,
audio graph, managed GPU/display/output color, delivery, interchange, cloud, and
all inference adapters remain roadmap work. The measured picture exporter and
short callback-clock probe are functional feasibility tools, with recorded limits.
No model has been integrated or benchmarked in EditBay yet.

R0 includes EB-001–003 and the measured ARM64 EB-004 prototype. EB-005 native
model feasibility is next; EB-006 reference jobs and EB-007 automation are open.
R1–R11 are planned, not shipped. No enterprise or GTM completion is declared.
