# Recovery without blocked input

Issue [#57](https://github.com/michaelmonetized/editbay/issues/57) follows source
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
UI metadata acceptance. Native and full workspace qualification are pending.
