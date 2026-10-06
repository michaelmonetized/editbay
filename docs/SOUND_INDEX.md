# Sound planning for longer cuts

A sound block now looks up only the paths that could contribute to its output
interval. The total number of past/future clips no longer consumes the block's
active-source, position or operation budget. Shared sound rendering, original
channel rules, PCM ownership and device callback behavior stay on the same path.

Compilation walks each source path backward. Timed nodes intersect its allowed
range; each clip maps child ranges back through its piecewise linear time map.
Forward and reverse segments use checked integer arithmetic. Bounds round outward
to enclosing integer frames at each level. Both endpoints are retained to cover
reverse endpoint ownership and fractional sample centers. Disabled tracks and
freeze-only paths have no active interval.

These ranges are conservative. They may retain extra work near boundaries, but
cannot replace the exact per-sample calculation. The existing evaluator still
checks every selected sample's ranges, direction, gain, nested timing and final
source position. Profile validation still covers every compiled path, including
inactive paths: indexing never permits an undeclared channel conversion.

A balanced interval tree stores each subtree's maximum end. A block query prunes
unreachable subtrees, deduplicates paths with multiple intervals and returns path
indices in original mix order. Floating-point accumulation order therefore stays
unchanged. Fully inactive paths can be absent from `SoundBlockPlan::sources()`;
possible contributors still carry explicit per-sample silence.

The query retains each selected path's enclosing span. Exact integer conversion
maps that closed frame interval to sample offsets inside the requested block.
Only those offsets run the temporal evaluator; all remaining slots are silence.
This avoids evaluating an entire block for a short cut near one edge. Multiple
intervals of one path use an enclosing union, with the evaluator deciding gaps.

Paths with one linear segment per clip and no animated node parameters can step
exact rational sample positions. The full evaluator resolves the first, second
and last center; all must be active with identical gain, direction and sample
step. Their exact affine prediction must match the last center. Checked integer
arithmetic fills the interior, preserving canonical fractions and float metadata
bits. Any failed proof or arithmetic bound falls back to full evaluation. With
single-segment maps, each node/clip range has a convex preimage, so active
endpoints cannot hide an inactive interior. Piecewise maps and animated gains
always use the full evaluator.

## Bounds and identity

Default `SoundBudget` distinguishes retained storage from playback work:

- 16,384 compiled paths, 1,048,576 total retained steps, 256 steps per path.
- 65,536 retained/intermediate intervals and 4,194,304 interval-build operations.
- At most 4,096 inspected tree nodes per block lookup.
- 64 possibly active paths, 4,096 output frames, 262,144 resolved positions and
  1,048,576 charged operations per block, including interval lookup.

Each bound fails visibly before native rendering. Conservative rounding can make
a very dense or heavily retimed graph exceed a block bound; it never silently
omits sound. The document's independent node/input limits also remain enforced.

Graph identity uses the `editbay-sound-graph-3` domain. Each shared source, timed
node and clip is hashed once, then path hashes are combined incrementally. Shared
mix inputs, animation and time maps are not copied once per path. Labels remain
outside sound identity; source interpretation, content hashes, ranges, operations,
tracks, timing and animation remain inside. Private snapshot ownership continues
to reject foreign plans and results even when public versions match.

`SoundSnapshot::index_stats()` exposes retained counts. A prepared block's `work()`
reports visited nodes, possible active paths, allocated sample slots, evaluated
positions, exact linear positions and charged operations. Operations
conservatively charge full path evaluation for the selected spans, while
the position budget still charges every allocated slot, including silence.

## Qualification

Core tests cover 1,024 sequential forward/reverse clips, exact cut boundaries,
nested reverse selections, fractional inverse bounds, extreme integer knots,
path deduplication, stable mix order and separate compilation/block budgets.
A 16,384-entry tree with a long overlapping interval tests pruning near its end.

`editbay-lab long-sound PROJECT REFERENCE_MASTER NEW_DIRECTORY CUTS` builds
65–256 linked one-frame edits through the production command path. It verifies
undo/redo, save/recovery, compares every original-channel PCM sample against
independently decoded master slices, records compilation/planning/render work and
plays the entire saved cut on the actual supervised device. Native window and
shared master checks follow separately. The fixture's 256-edit cap respects the
current canonical cut's single mix-node input limit; it is not the index's path
capacity.

The predeclared block-planning p95 remains 5 ms for 4,096 frames. Compilation's
observed process high-water limit is 256 MiB for these declared sound workloads;
active native edit input remains 50 ms and cancellation remains two seconds.
The camera planning miss retained under PR #33 is not erased by this work.
Physical audibility, two-hour hardware drift, sustained video performance and
full roadmap/release acceptance require their own evidence.
