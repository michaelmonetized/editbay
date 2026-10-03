# Omadesign reuse and continuity

Inspected 2026-10-03 at
`/home/michael/Projects/omadesign/.worktrees/release-063`, commit `c4813ef9`.
That checkout declares Omadesign 0.6.3. The primary directory was on an older
feature branch; the release worktree was the architecture reference. This is
source inspection, not a fresh production cloud or user-count audit.

## Reuse map

| EditBay requirement | Actual Omadesign reference | Adaptation |
| --- | --- | --- |
| Native application | `Cargo.toml`, `src/app.rs`, `src/ui/theme.rs`, `vendor/` | eframe/egui, fonts/theme, applicable input/frame-loop fixes; extract limited components |
| Durable manual save | `src/project.rs::save_to` | Unique temp, complete write, sync, rename; add parent sync and writer coordination |
| Background recovery | `src/app/recovery.rs`, `src/app/tabs.rs::finish_recovery` | Idle debounce, worker preparation, revision ownership, stale-result rejection |
| Recovery storage | `src/project.rs::prepare_swap`, `PreparedSwap` | Stable IDs, recovery metadata, atomic publication; video manifests reference persistent assets |
| Welcome/catalog | `src/ui/welcome.rs`, `src/ui/welcome/catalog.rs`, `docs/welcome-screen.md` | Your Work/create/Projects, natural-aspect previews, Recovered, asynchronous recursive scanning |
| Team visibility | `src/ui/welcome/team.rs` | Show Team only for a connected account with actual shared projects |
| Startup/dialogs | `src/app/startup.rs`, `src/app/file_dialogs.rs` | Atomic preferences, remembered entry state, asynchronous dialogs with document ownership |
| Project brands | `src/brand.rs`, `src/ui/library/brand.rs` | Portable `.omabrand`, fonts, palettes, marks; extend through versioned shared asset contracts |
| Layered/vector migration | `src/formats/`, `docs/format-support.md`, `docs/MANUAL.md` | PSD/PSB, SVG, PDF, OpenRaster, Lottie, conversion notes; preserve original sources |
| Cloud transport | `src/cloud/client.rs`, `src/app/cloud.rs`, `docs/cloud.md` | Worker transfers, device auth, versioned source/assets, owner/editor/reviewer roles |
| Acquisition/retention | Welcome, docs, showcase, cloud review, project asset bank | Familiar first run; bring existing work; complete client workflows and reusable libraries |
| Native ML shipping | `assets/models/README.md`, `vendor/ml-notices/` | Verified native inference artifacts, model hashes/licenses, no Python required by users |
| Release discipline | `scripts/release.sh`, `scripts/install-remote.sh`, release QA docs | Local Rust builds/packages, artifact hashes, clean install/update, separate web verification |

## Preserve the product behavior

The welcome reference has three columns: Your Work, central creation/help, and
Projects. File browsers use natural-aspect masonry with at most three columns,
hover names, single-click open, Shift-selection, and explicit multi-open. Keep
fast keyboard access and small-window layouts. Use Omarchy's palette and fonts;
the application should feel like part of the same creative family.

Scan real work rather than only a recent-path list. Follow nested projects;
skip hidden/trash/symlink traversal while recognizing `.omabrand` markers. Keep
partial/error states visible. EditBay adds video covers, sequence/job status,
missing-media notices, and versioned recovery previews.

Use Omarchy's provider-neutral agent integration and version-matched docs for
Learn/Create actions. Preserve user choice rather than hard-code a model vendor.
Commands should create native editable results and inspect actual render output.

## Share concepts without coupling releases

Start with adapters and fixtures; extract a shared crate only when both apps use
it. Retain independent IDs, preferences, cache/recovery roots, releases, and cloud
document types. Existing `.omabrand` files remain valid. Shared client/project
context requires collision rules and explicit association.

Omadesign's cloud docs describe explicit transfers, immutable review snapshots,
and member roles. They expressly exclude live multi-user canvas editing. Adopt
that truthful boundary initially. Its documented 100 MB source/asset limit and
still-image review model require substantial expansion for EditBay media.

Some Omadesign bridges have external runtimes and licenses. Reusing a supported
format label is not permission to introduce Python into EditBay. Reuse native
codecs; re-evaluate optional external conversions separately under this project's
no-Python production contract.

The user reports an existing audience of hundreds. Treat that as an opt-in
adoption channel and a source of professional workflow interviews. Do not infer
that all users are video professionals or contact/import their accounts without
an explicit product connection and communication authorization.
