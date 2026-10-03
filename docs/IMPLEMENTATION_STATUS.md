# Rust restart: implementation status

Validated locally on 2026-10-03, Linux ARM64, Rust/Cargo 1.98.0.
This is the initial project/recovery foundation, not a finished media editor.

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

## Validation

| Check | Result |
| --- | --- |
| `cargo test --workspace --locked --offline` | 18 passed; 0 failed or ignored |
| Core regression suite | 16 tests: identity/rational time, validation, round trips, permission preservation, concurrent/stale writers, corruption, source loss and overwrite refusal |
| CLI binary integration suite | 2 tests: complete create/edit/checkpoint/recovery flow and invalid-command/name behavior; paths contain spaces |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `./bin/editbay --version` | `editbay 0.1.0 (Rust project foundation)` |

The tests exercise actual local files and the actual CLI executable. They are
software evidence. A power-loss test, a native editor process-kill/restart test,
and physical-device/hardware qualification have not been performed.

## Still to implement

Background autosave scheduling/ownership, checkpoint pruning, welcome/catalog UI,
native desktop shell, timeline/undo/composition schema, ingest/playback/audio,
GPU/color/render/export, interchange, cloud, and all inference adapters remain
roadmap work. No model has been integrated or benchmarked in EditBay yet.

R0 is partially implemented through EB-001–003. Its engine/model feasibility and
reference-project work are outstanding. R1–R11 are planned, not shipped.
