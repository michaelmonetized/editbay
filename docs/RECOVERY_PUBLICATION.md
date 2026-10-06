# Recovery without blocked input

Issue [#57](https://github.com/michaelmonetized/editbay/issues/57) / draft
[PR #58](https://github.com/michaelmonetized/editbay/pull/58) follows source
preparation in [PR #56](https://github.com/michaelmonetized/editbay/pull/56).
The initial native camera range-job trace recorded a 236.380 ms checkpoint commit
inside a 239.610 ms UI frame. A viewer input took 157.870 ms. The unchanged repeat
passed, but synchronous directory synchronization remained a real input hazard.

One filesystem worker now owns preparation, publication and temporary-file
cleanup for the complete checkpoint lifetime. It prepares synchronized bytes,
reports readiness and waits off the UI thread. The UI checks the tab, path
generation, project identity and recovery revision, then sends a bounded approval.
The worker claims publication atomically against cancellation before linking and
synchronizing the immutable checkpoint. The four-job limit includes workers
waiting for approval or completing publication.

Save and close cancel unclaimed publication without waiting. Workspace destruction
also wakes and cancels waiting workers. If publication wins the atomic race first,
that authorized immutable snapshot finishes; the UI still rejects an obsolete
acknowledgement. Edits can continue while an older revision is being protected,
and remain dirty until their own recovery/save succeeds. No original is replaced.
Failures preserve older valid history, report a visible error and retain retry
scheduling. The 1 s idle / 10 s maximum recovery policy is unchanged.

Local tests cover cancellation before/after approval, abandoned lifetime cleanup,
blocked publication without blocked cancellation, four waiting workers, close,
manual save, publication failure with older history retained, retry and one hundred
inactive/untitled documents. Diagnostics distinguish worker filesystem time from
UI metadata acceptance and expose each bounded worker's preparing, awaiting
approval, approved or publishing phase without filesystem access.

Frozen `bca4f46` passes full workspace checks, native camera/six-channel range-job
flows, real filesystem-error handling and recovery scheduling. Continuous edits
checkpoint within 9874.232 ms; idle publication takes 1149.274 ms. One actual
108.580 ms filesystem commit is accepted on the UI in under 1 microsecond.
The first hundred-trial attempt times out awaiting the active checkpoint on trial
1; its inactive checkpoint was acknowledged and the UI continued responding.
The exact worker stage was not captured, so the cause remains unproven. A
two-trial reproduction passes.

The next attempt completes six trials, then exposes a driver defect: a historical
revision-0 event in which all checkpoints were current allowed killing the app
before revision 3 was protected. Revision 3 was never acknowledged in that trace.
The driver now requires the exact captured project IDs/revisions and an event no
earlier than the final edit. Its regression test rejects the stale event. Both
failed attempts remain; new stage diagnostics and native qualification continue.
