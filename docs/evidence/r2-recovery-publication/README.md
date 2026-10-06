# Recovery publication evidence

Issue #57 / PR #58, based on #56. Production `bca4f46` moves checkpoint
publication and cleanup off the UI. `04ac35983ae82577685cf4f55cbd0b8ff1f9f092`
adds bounded worker-phase diagnostics and exact checkpoint acknowledgements in
the lab. `829ba9a79859df79ee7630d2311df5edac9c219f` changes only picker input
focus in the lab. Frozen binary/Cargo.lock hashes are in each candidate's
SHA256SUMS. Final trials use the `observed` application and `picker-driver` lab.
Linux ARM64, Apple M1 Pro, installed Hyprland/Wayland and native Flea portal.

Final `picker-driver/workspace/qualification.json` passes all 100 trials: fifty
saved and fifty untitled projects, active revision-3 and inactive recovery,
process kill, native recovery/save/reopen, unchanged originals and independent
recovered project identities. The catalog contains 4,000 documents. Across 250
inputs, p95 acceptance is 18.904 ms and maximum 32.913 ms. Across 200 checkpoint
acknowledgements, worker commit p95 is 3.421 ms and maximum 28.874 ms; UI metadata
acceptance p95 is 0.003 ms and maximum 0.008 ms. CPU frame-work p95 is 4.277 ms.
These are software-injected input and process/UI timings, not physical latency.

`observed` also passes continuous-edit publication within 9826.256 ms, idle
publication in 1045.501 ms, real permission/full-filesystem errors with visible
failure and successful retry, and both native camera/six-channel range-job
workflows. Viewer-input p95 is 25.706 / 16.099 ms. Both 1440×900 exported-result
captures were visually inspected. Initial `candidate` timing also records a
108.580 ms filesystem commit with UI acceptance under one microsecond.

Earlier attempts are retained:

- `candidate/workspace`: trial 1 times out awaiting an active checkpoint. The
  inactive checkpoint was acknowledged and the UI responded; the precise worker
  stage was not captured. Cause remains unproven. A two-trial reproduction passes.
- `candidate/repeat`: six trials finish, then the lab kills before revision 3 is
  acknowledged because a historical revision-0 event satisfies its predicate.
  The corrected driver requires exact IDs/revisions after the final edit, with
  a regression test rejecting stale events. No acknowledged revision was lost.
- `observed/workspace`: fourteen trials finish; trial 14's active/inactive
  checkpoints are durable, then the native picker input driver fails after
  recovery. `picker-driver` reasserts owned focus and acknowledges list focus
  before navigation. The unchanged application then passes all 100 fresh trials.

`checks/observed-workspace` records 255 default / 256 all-feature tests and seven
doc tests each, all-target/all-feature Clippy with warnings denied, and fmt.
`checks/picker-driver-build.log` records final lab Clippy/build. Original focused
failures and corrections remain. No sustained audio, physical audibility, other
hardware, completed-client or independent-user gate is closed here.

Archives omit executable binaries, media outputs and repeated 4,000-document
catalog fixtures. Their qualification hashes/metadata remain. Original projects,
recovery checkpoints, native captures and logs are retained. JSONL traces are
losslessly gzip-compressed; root SHA256SUMS covers archived files except itself.
