# Native document automation

EB-007 contract, 2026-10-03. Automation and the CLI use `editbay-core`'s typed
document commands. The R1 native UI uses the same editor; there is no separate
agent document format or provider-specific authoring path.

## Start a scoped server

```sh
cargo run -p editbay-automation --locked --bin editbay-mcp -- \
  '/path/to/Job.editbay' '/path/to/Recovery'
```

The caller grants access by selecting an existing regular project file and a
recovery directory at startup. RPC arguments cannot supply other file paths.
The server creates the recovery directory when absent. Project and root symlinks
are rejected; the recovery catalog skips symlink entries. Paths are canonicalized
once. This is a local stdio service, with no HTTP listener, authentication service,
credential access, source upload, or assistant-provider dependency. Filesystem
operations run on blocking worker threads; serialized document transactions keep
the async protocol thread responsive.

The pinned official [Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
is `rmcp 3.5.0`. Actual child-process tests exercise discovery/per-request metadata
under protocol `2026-07-28` and legacy initialize under `2025-11-25`. Protocol
generation handling belongs to the SDK; EditBay's document rules are identical.
Stdout contains protocol messages only; startup errors go to stderr.

## Implemented tools

| Tool | Behavior |
| --- | --- |
| `inspect_document` | Current validated document/version, undo/redo labels, granted paths, disk match/change/unavailable state and explicit capabilities |
| `apply_commands` | Typed 1..64-command group with a 1..256-byte label; validate the whole group, publish one revision and one undo step, durably save |
| `undo` / `redo` | Restore the previous/next group content, advance revision, durably save |
| `create_checkpoint` | Require current version; write a new immutable checksummed snapshot into the granted recovery directory |
| `inspect_recoveries` | List valid and corrupt snapshots in that directory without changing them |

`rename_project` is the only current document command. Its schema is derived from
the core Rust type. Preview, export, media import, jobs and cloud capabilities are
false, and corresponding tools are absent. The lab's actual decode/inference/
export probes are documented separately; their presence does not enable editor
workflows or turn them into document jobs.

Example `apply_commands` arguments, using the identity/revision returned by inspection:

```json
{
  "expected": {"project_id": "5502bb7f-0f37-4e30-b561-5e08455543c7", "revision": 7},
  "label": "Rename job",
  "commands": [{"kind": "rename_project", "name": "Approved cut"}]
}
```

## Ownership, failure and undo

Every mutation includes `expected.project_id` and `expected.revision`. A mismatch
fails before publication. Unknown commands/fields are rejected. A group is
validated on a candidate document; invalid intermediate commands reject the
whole group. A group whose final content is unchanged consumes no revision or
history entry. Undo/redo always creates a newer revision, so an old worker result
cannot become current again after undo. Revision overflow fails unchanged.

Saved document edits use the existing writer lock and loaded-document comparison.
The session replaces its document/history only after `save_if_unchanged` succeeds.
Outside edits, deleted files, permissions and destination conflicts return visible
tool errors. No stale save recreates a deleted original. Inspection keeps the
in-memory version visible even when the disk file is unavailable, allowing a
checkpoint and separate-copy recovery. Recovery never replaces the original.

History is bounded to 128 groups in memory and starts empty after reopening;
persisted project/recovery state remains available. Restart does not promise
persistent undo history. A disconnected caller can lose a response after a save
commits: inspect the current version before retrying. Expected revisions reject
duplicate retries; cancellation is not a rollback after durable publication.

Domain failures return `isError: true` with the actual explanation. Invalid RPC
arguments and unknown tool names remain SDK protocol errors. Tool annotations
describe behavior; scope and revision checks enforce it.

## Contracts for subsequent milestones

These are implementation requirements, not advertised tools:

- **Sources:** grant explicit read/write roots or exact assets. File identities,
  byte hashes and media stream interpretation are retained. Source bytes are
  immutable; import/copy writes a new destination. An agent cannot widen its own
  permissions. Missing/unreadable sources remain visible. Network/model downloads
  need their own approved source/terms record.
- **Time:** source, sequence and composition boundaries use distinct integer or
  rational types. A nanosecond display value never becomes a floating-point edit
  boundary. Inspect both source mapping and sequence placement after retiming.
- **Jobs:** a stable job ID identifies kind, queued/running/cancelling/completed/
  cancelled/failed state, completed/total work, error, owner project/revision,
  source hashes, graph/color configuration and model/runtime version. Cancellation
  must stop the underlying codec/inference process and clean unpublished outputs.
  Retry/resume verifies source and configuration ownership. A stale job cannot
  mutate the newer document; committing a result uses a normal expected-version
  command. Progress is measured work, never a timer simulating completion.
- **Inspection:** viewer/export inspection must return actual evaluated pixels or
  samples plus frame/source time, dimensions, channels, alpha convention, color
  configuration and result ownership. Export inspection reopens the produced file
  and verifies metadata/duration/pictures/sound. A document summary or success flag
  cannot stand in for rendered output.
- **Provider independence:** the artist's chosen Omarchy agent issues these typed
  commands. Production SAM/matting/transcription models remain separate managed
  dependencies. Manual authoring, correction and cached results work offline
  without an assistant or an inference download.

R2/R3/R4 add these capabilities only when complete document, worker, undo,
recovery and actual-output paths exist. Tests must cover stale results after
save/close/replacement, failure/cancel cleanup and source preservation.
