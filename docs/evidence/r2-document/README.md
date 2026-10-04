# R2 document layer evidence

Local ARM64, Apple M1 Pro, Linux/Omarchy, Rust/Cargo 1.98.0, 2026-10-03. This is
the EB-020 document/command slice with safe schema migration and typed frame-plan
inspection. R2 ingest/playback/render/audio/cache/job and hardware release gates
remain open. No production picture/sound or enterprise/GTM completion is claimed.

## Provenance

The unoptimized native application SHA-256 is
`4358d10899a29f55c7a8075275039228ff82a83c302b1cb7513b3c3eb4932f28`;
CLI `2348b4561c9830fb2c8b41910ab8ae7b8adc1a770079e5b82acc02ed1f116904`;
final native qualification driver
`7fed16bdffd33b815bc87b366a9ddb393c4b431f15ed8856bc3ec8a036275424`.
Lockfile `a1d94e1c574381fd4f952bbd4516171b675dc9eb1b4a7211c8f229fc088c96f9`.
`source-hashes.txt` identifies changed compiled inputs and the old checkpoint
fixture; the containing commit is the source revision. Binaries/private traces
remain in `/var/tmp/editbay-r2-document/`, outside Git.

## Software checks

`cargo test --workspace --locked`: 78 tests plus five SDK documentation examples,
zero failed or ignored. `cargo test --workspace --all-features --locked`: 79 plus
five examples, with verified native libtorch selected. The full all-feature
Clippy and fmt checks pass. A subsequent driver-only change was checked by full
Clippy, its three real worker/inventory tests and actual native trials.

The eight new core tests cover editable multi-stream/VFR/six-channel/nested/masked/
animated document data, command undo/save/recovery, graph/socket/cycle/link/identity
failures, exact retimes and source ordinals, dependency fingerprints, immutable
worker snapshots, legacy checkpoint integrity/migration and actual 64 MiB limits.
Two new real CLI tests and one new real MCP process test exercise authored scene
commands, frame inspection, stale rejection, undo/redo/recovery and separate-file
migration. The synthetic source fields in invariant tests are explicitly marked;
they do not claim decoded media.

## Reviewable authored data

`fixture.editbay` was created and edited through the frozen real CLI. The saved
document contains two reusable compositions: straight linear float solids,
polygon/feathered mask, animated transform, over composition and a reverse nested
clip. The command group, durable receipt, child frame 24 and parent frame 0 plans
are included. Child frame 24 evaluates X translation to 96 pixels. Parent frame 0
retains the exact reverse child boundary at 48. These are graph/time/parameter
results, not rendered frames.

The original authored file hash is
`01615088bcf370fc914cc2de0b2ed9729de63494de4186c967bbfbed726637a0`.
A CLI checkpoint/recovery produced an independent project ID, retained revision 1
and all graph data, and retained the child frame-plan hash
`451a76e94d25277cda2ee25aa01ac7ff38b9ac26662d3609924d6520809040de`.
The recovered file hash is
`6d9062c824733106ff2e4b35c93f8b951f6097b8b7eb6e26fae24f7a07e842f6`.

`migration-report.json` records real separate-file conversion of the immutable
schema 1 reference. The original hash remains
`8c843edab47076ccb60b31fd8c45e70d549a352ec9fff0fb4c98b91b27ad67a1`;
schema 2 output is
`56d3cc8490dae96922835074f911a2d485bfe2fff4fe0a8239b2c3a2be5a56a3`.
Identity, revision 7 and all three rational profiles are retained. Existing
destinations are refused. The legacy checkpoint fixture hash is
`11592552e548bebf12d0fc08e8a0a84085a995a0089dc95ac79951778ee00245`;
it was produced by the R1 CLI
`752250c4261a3275ec1579c45bdde9ad963ee378028662bac0c15675147d1a4c`.
Its integrity is checked before migration; corruption cannot publish a copy.

## Native regression

The first run completed 28 native trials, then a missing Undo key event stopped
the driver. The trace contains no delivered Z key; it does retain both dirty
documents and durable revision 1/0 checkpoints. That incomplete run remains local
and is not counted as a completed qualification. The driver now verifies owned
application focus before each application action and uses 8 ms persistent-device
key pulses. It does not retry a missed action or fabricate an acknowledgement.
The application binary is unchanged. A fresh three-trial smoke passed, followed
by the full qualification: **100/100 trials** with 4,000 catalog documents, 50
untitled projects and 50 saved originals. Active and inactive acknowledged
checkpoints survive process termination, recover independently through the native
portal, and reopen with preserved content and unchanged originals.

`qualification.json` records 250 inputs: p50 11.179 ms, p95 22.256 ms, maximum
33.244 ms. The 200 UI checkpoint commits measure p50 0.219 ms, p95 0.564 ms,
maximum 4.204 ms. Across 15,889 CPU frames, p50 is 0.855 ms, p95 3.846 ms and
maximum 85.527 ms. Peak RSS/HWM is 101,360 KiB. These are unoptimized local
workspace measurements, not production playback measurements.

`graph-native.png` shows the authored schema 2 document, both compositions and
its sequence in the actual 1440x900 native workspace. The application correctly
reports that composition preview is unavailable. Native Save preserves its exact
file hash; Checkpoint now acknowledges revision 1. Recovery of that native
checkpoint retains both graphs and the frame-24 plan hash, under a new project ID.
`native-graph-recovery.json` records that result. `legacy-native.png` shows the
separate-file migrated project and its original profiles, including the portrait
aspect. Both files retain the hashes above after native inspection. The singular
track/node caption is a minor display defect recorded for the following media UI
layer; it does not change the frozen qualification artifact.

Actual source decoding, multi-channel sound evaluation, displayed/exported pixels,
long playback/seek/drift, physical input/power loss, additional GPUs and independent
client jobs remain the documented production gates.
