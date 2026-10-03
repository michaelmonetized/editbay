# EditBay

A native Rust production suite for Omarchy: editing, motion graphics, VFX, color,
sound, and connected client/team workflows. The goal is to let professionals move
their existing production work from macOS/Windows to Omarchy.

The Rust implementation currently provides **project and recovery CLI commands**.
The desktop editor and media engine are not implemented yet.
The application and production workers use Rust with native dependencies.

## Roadmap and decisions

- [Complete implementation roadmap](docs/ROADMAP.md): stable issue IDs, dependencies,
  all 18 gap areas, production AI, and complete-job acceptance gates.
- [Implementation status and validation](docs/IMPLEMENTATION_STATUS.md): what works
  in the Rust rewrite today and what remains unimplemented.
- [Rust architecture](docs/ARCHITECTURE.md): native UI, typed compositions, GPU/color,
  audio, jobs, recovery, inference, and cloud boundaries.
- [Omadesign reference](docs/OMADESIGN_REFERENCE.md): actual 0.6.3 source locations
  for welcome, recovery, projects, assets, cloud, and native ML shipping.
- [AI models](docs/AI_MODELS.md): Meta-first candidates, capabilities, licenses,
  portrait/video matting, offline edge refinement, optional local vision review,
  native runtime feasibility, tracking, and rigging.
- [Migration and adoption](docs/MIGRATION_AND_GTM.md): existing-project migration,
  client workflows, cross-app assets, user acquisition, and earned retention.

## Run the Rust foundation

Build on the local machine with Rust/Cargo:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
./bin/editbay --help
./bin/editbay new /path/to/client/cut.editbay "Client spot"
./bin/editbay info /path/to/client/cut.editbay
./bin/editbay rename /path/to/client/cut.editbay "Client spot revised"
./bin/editbay checkpoint /path/to/client/cut.editbay /path/to/recovery
./bin/editbay recoveries /path/to/recovery
./bin/editbay recover /path/to/checkpoint /path/to/Recovered.editbay
```

The executable is `target/debug/editbay`; the source launcher builds/runs that
Rust command with the lockfile. Projects use a fresh validated schema and stable
UUID identity. Frame rates stay rational. Saves use synchronized atomic writes
with writer locks and stale-edit rejection for edits. New files are published
without replacing an existing destination. Checkpoints are immutable and integrity
checked; recovery creates an independent copy and refuses to replace the original
or an existing destination. Invalid checkpoints stay visible as errors alongside
older valid work.

This foundation does not yet implement background autosave scheduling, a welcome
UI, media playback/export, timeline commands, cloud, or inference. Those have
explicit implementation and verification gates in the roadmap.
