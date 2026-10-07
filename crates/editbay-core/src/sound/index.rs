use super::invalid;
use crate::{Clip, FrameRange, FrameRate, Result, SourcePosition};
use std::ops::Range;

const MAX_BUILD_OPERATIONS: usize = 4_194_304;
const MAX_LOOKUP_NODES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub start: i64,
    pub end: i64,
}

impl Span {
    /// Enclose a document range with signed, inclusive index boundaries.
    /// `range` is in composition frames; returns checked endpoints.
    pub fn range(range: FrameRange) -> Result<Self> {
        Ok(Self {
            start: i64::try_from(range.start).map_err(|_| invalid("sound index range overflow"))?,
            end: i64::try_from(range.end).map_err(|_| invalid("sound index range overflow"))?,
        })
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start <= end).then_some(Self { start, end })
    }
}

pub(super) struct Builder {
    remaining: usize,
    limit: usize,
}

impl Builder {
    /// Bound temporary intervals and total inverse-mapping work.
    /// `limit` caps each interval vector; returns a fresh compilation counter.
    pub fn new(limit: usize) -> Self {
        Self {
            remaining: MAX_BUILD_OPERATIONS,
            limit,
        }
    }

    fn charge(&mut self) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| invalid("sound interval compilation exceeds its operation budget"))?;
        Ok(())
    }

    fn push(&mut self, spans: &mut Vec<Span>, span: Span) -> Result<()> {
        if spans.len() >= self.limit {
            return Err(invalid(
                "sound interval compilation exceeds its interval budget",
            ));
        }
        spans.push(span);
        Ok(())
    }

    /// Restrict possible activity to a timed node without losing its endpoints.
    /// `spans` is unrestricted or previously bounded; `range` is the node extent.
    /// Returns closed enclosing intervals under the compilation work limit.
    pub fn intersect(&mut self, spans: Option<Vec<Span>>, range: FrameRange) -> Result<Vec<Span>> {
        let range = Span::range(range)?;
        let Some(spans) = spans else {
            return Ok(vec![range]);
        };
        let mut output = Vec::new();
        for span in spans {
            self.charge()?;
            if let Some(span) = span.intersection(range) {
                self.push(&mut output, span)?;
            }
        }
        Ok(output)
    }

    /// Move conservative child-time intervals back through a clip's time map.
    /// `spans` is unrestricted or closed child-coordinate bounds; `clip` supplies
    /// exact integer knots. Returns enclosing parent-frame intervals, including
    /// both endpoints so reverse ownership and fractional sample centers survive.
    pub fn preimage(&mut self, spans: Option<Vec<Span>>, clip: &Clip) -> Result<Vec<Span>> {
        let unbounded = spans.is_none();
        let parent = Span::range(clip.range)?;
        let mut output = Vec::new();
        for points in clip.time_map.points.windows(2) {
            self.charge()?;
            let a = points[0];
            let b = points[1];
            if a.source_tick == b.source_tick {
                continue;
            }
            let source = Span {
                start: a.source_tick.min(b.source_tick),
                end: a.source_tick.max(b.source_tick),
            };
            let unrestricted = [source];
            let spans = spans.as_deref().unwrap_or(&unrestricted);
            for span in spans {
                self.charge()?;
                let scaled;
                let span = if unbounded {
                    span
                } else {
                    let scale = i64::from(clip.time_map.source_denominator);
                    scaled = Span {
                        start: span
                            .start
                            .checked_mul(scale)
                            .ok_or_else(|| invalid("sound source denominator overflow"))?,
                        end: span
                            .end
                            .checked_mul(scale)
                            .ok_or_else(|| invalid("sound source denominator overflow"))?,
                    };
                    &scaled
                };
                let Some(span) = span.intersection(source) else {
                    continue;
                };
                let denominator = (i128::from(b.source_tick) - i128::from(a.source_tick)).abs();
                let local = |bound: i64, ceil: bool| -> Result<i64> {
                    let distance = (i128::from(bound) - i128::from(a.source_tick)).abs();
                    let numerator = distance
                        .checked_mul(i128::from(b.frame - a.frame))
                        .ok_or_else(|| invalid("sound index inverse overflow"))?;
                    let offset =
                        numerator / denominator + i128::from(ceil && numerator % denominator != 0);
                    i64::try_from(i128::from(parent.start) + i128::from(a.frame) + offset)
                        .map_err(|_| invalid("sound index parent overflow"))
                };
                let (low, high) = if b.source_tick > a.source_tick {
                    (span.start, span.end)
                } else {
                    (span.end, span.start)
                };
                let interval = Span {
                    start: local(low, false)?,
                    end: local(high, true)?,
                };
                if let Some(span) = interval.intersection(parent) {
                    self.push(&mut output, span)?;
                }
            }
        }
        normalize(&mut output);
        Ok(output)
    }

    /// Enclose source time reached by bounded parent intervals through a clip.
    /// `spans` owns inclusive parent-frame bounds, `clip` supplies exact knots,
    /// and `cancelled` interrupts planning. Returns enclosing integer source
    /// ticks; reverse segments retain both endpoints and freezes contribute silence.
    pub fn image(
        &mut self,
        spans: Vec<Span>,
        clip: &Clip,
        cancelled: &mut impl FnMut() -> bool,
    ) -> Result<Vec<Span>> {
        let parent = Span::range(clip.range)?;
        let mut output = Vec::new();
        for points in clip.time_map.points.windows(2) {
            self.charge()?;
            if cancelled() {
                return Err(invalid("sound source planning cancelled"));
            }
            let a = points[0];
            let b = points[1];
            if a.source_tick == b.source_tick {
                continue;
            }
            let segment = Span {
                start: parent
                    .start
                    .checked_add(
                        i64::try_from(a.frame)
                            .map_err(|_| invalid("source planning frame overflow"))?,
                    )
                    .ok_or_else(|| invalid("source planning range overflow"))?,
                end: parent
                    .start
                    .checked_add(
                        i64::try_from(b.frame)
                            .map_err(|_| invalid("source planning frame overflow"))?,
                    )
                    .ok_or_else(|| invalid("source planning range overflow"))?,
            };
            for span in &spans {
                self.charge()?;
                if cancelled() {
                    return Err(invalid("sound source planning cancelled"));
                }
                let Some(span) = span
                    .intersection(segment)
                    .and_then(|span| span.intersection(parent))
                else {
                    continue;
                };
                let mapped = |frame: i64| -> Result<SourcePosition> {
                    clip.time_map.position_validated(SourcePosition::new(
                        frame
                            .checked_sub(parent.start)
                            .ok_or_else(|| invalid("source planning local time overflow"))?,
                        1,
                    )?)
                };
                let first = mapped(span.start)?;
                let last = mapped(span.end)?;
                let floor = |position: SourcePosition| -> Result<i64> {
                    i64::try_from(
                        i128::from(position.numerator).div_euclid(i128::from(position.denominator)),
                    )
                    .map_err(|_| invalid("source planning floor overflow"))
                };
                let ceil = |position: SourcePosition| -> Result<i64> {
                    let n = i128::from(position.numerator);
                    let d = i128::from(position.denominator);
                    i64::try_from(n.div_euclid(d) + i128::from(n.rem_euclid(d) != 0))
                        .map_err(|_| invalid("source planning ceiling overflow"))
                };
                self.push(
                    &mut output,
                    Span {
                        start: floor(first)?.min(floor(last)?),
                        end: ceil(first)?.max(ceil(last)?),
                    },
                )?;
            }
        }
        normalize(&mut output);
        Ok(output)
    }
}

fn normalize(spans: &mut Vec<Span>) {
    spans.sort_unstable_by_key(|span| (span.start, span.end));
    let mut count = 0;
    for read in 0..spans.len() {
        let span = spans[read];
        if count > 0 && span.start <= spans[count - 1].end {
            spans[count - 1].end = spans[count - 1].end.max(span.end);
        } else {
            spans[count] = span;
            count += 1;
        }
    }
    spans.truncate(count);
}

struct Entry {
    span: Span,
    leaf: usize,
    maximum: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Candidate {
    pub leaf: usize,
    span: Span,
}

impl Candidate {
    /// Bound block offsets by the path's closed root-frame interval.
    /// `first`, `frames`, `rate` and `fps` describe exact output sample centers.
    /// Returns the enclosing sample range, clamped before narrowing to offsets.
    pub fn samples(&self, first: u64, frames: u32, rate: u32, fps: FrameRate) -> Range<usize> {
        let scaled = |frame: i64| {
            u128::try_from(frame).unwrap_or(0) * 2 * u128::from(rate) * u128::from(fps.denominator)
        };
        let numerator = u128::from(fps.numerator);
        let divisor = numerator * 2;
        let start = scaled(self.span.start)
            .saturating_sub(numerator)
            .div_ceil(divisor);
        let end = if self.span.end < 0 || scaled(self.span.end) < numerator {
            0
        } else {
            (scaled(self.span.end) - numerator) / divisor + 1
        };
        let first = u128::from(first);
        let limit = first + u128::from(frames);
        (start.clamp(first, limit) - first) as usize..(end.clamp(first, limit) - first) as usize
    }
}

#[derive(Default)]
pub(super) struct Index {
    entries: Vec<Entry>,
}

impl Index {
    /// Retain a path's conservative ranges under a shared storage limit.
    /// `leaf` identifies mix order; `spans` supplies ranges; `limit` caps entries.
    /// Returns an error before exceeding the declared index allocation count.
    pub fn add(&mut self, leaf: usize, spans: Vec<Span>, limit: usize) -> Result<()> {
        if self
            .entries
            .len()
            .checked_add(spans.len())
            .is_none_or(|count| count > limit)
        {
            return Err(invalid("compiled sound exceeds its interval budget"));
        }
        self.entries.extend(spans.into_iter().map(|span| Entry {
            span,
            leaf,
            maximum: span.end,
        }));
        Ok(())
    }

    /// Build subtree maxima after all bounded path ranges are retained.
    /// Takes no arguments; leaves a balanced, immutable query layout.
    pub fn finish(&mut self) {
        self.entries
            .sort_unstable_by_key(|entry| (entry.span.start, entry.span.end, entry.leaf));
        build(&mut self.entries);
    }

    /// Inspect retained interval storage.
    /// Takes no arguments and returns the number of tree entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Query the balanced interval index with exact sample-center endpoints.
    /// `first`, `last` and `limit` bound the block and active paths. Returns unique
    /// paths and enclosing spans in mix order, and the inspected tree node count.
    pub fn query(
        &self,
        first: SourcePosition,
        last: SourcePosition,
        limit: usize,
    ) -> Result<(Vec<Candidate>, usize)> {
        let mut found = Vec::new();
        let mut visited = 0;
        query(&self.entries, first, last, limit, &mut found, &mut visited)?;
        Ok((found, visited))
    }
}

fn build(entries: &mut [Entry]) -> i64 {
    if entries.is_empty() {
        return i64::MIN;
    }
    let middle = entries.len() / 2;
    let (left, right) = entries.split_at_mut(middle);
    let (entry, right) = right.split_first_mut().expect("nonempty interval tree");
    entry.maximum = entry.span.end.max(build(left)).max(build(right));
    entry.maximum
}

fn query(
    entries: &[Entry],
    first: SourcePosition,
    last: SourcePosition,
    limit: usize,
    found: &mut Vec<Candidate>,
    visited: &mut usize,
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    *visited += 1;
    if *visited > MAX_LOOKUP_NODES {
        return Err(invalid("sound interval lookup exceeds its node budget"));
    }
    let middle = entries.len() / 2;
    let entry = &entries[middle];
    if first.compare_tick(entry.maximum)?.is_gt()
        || last.compare_tick(entries[0].span.start)?.is_lt()
    {
        return Ok(());
    }
    query(&entries[..middle], first, last, limit, found, visited)?;
    if first.compare_tick(entry.span.end)?.is_le() && last.compare_tick(entry.span.start)?.is_ge() {
        match found.binary_search_by_key(&entry.leaf, |candidate| candidate.leaf) {
            Ok(position) => {
                let span = &mut found[position].span;
                span.start = span.start.min(entry.span.start);
                span.end = span.end.max(entry.span.end);
            }
            Err(position) => {
                if found.len() >= limit {
                    return Err(invalid("sound block exceeds its active source budget"));
                }
                found.insert(
                    position,
                    Candidate {
                        leaf: entry.leaf,
                        span: entry.span,
                    },
                );
            }
        }
    }
    if last.compare_tick(entry.span.start)?.is_ge() {
        query(&entries[middle + 1..], first, last, limit, found, visited)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClipSource, TimeMap, TimePoint};
    use uuid::Uuid;

    #[test]
    fn candidate_offsets_match_exact_centers_at_fractional_rates_and_boundaries() {
        for (rate, fps) in [
            (48000, FrameRate::new(24, 1).unwrap()),
            (44100, FrameRate::new(24000, 1001).unwrap()),
            (8000, FrameRate::new(1000, 1).unwrap()),
        ] {
            for span in [
                Span { start: -4, end: -1 },
                Span { start: 0, end: 0 },
                Span { start: 1, end: 3 },
                Span { start: 4, end: 4 },
                Span { start: 7, end: 11 },
                Span {
                    start: i64::MAX - 1,
                    end: i64::MAX,
                },
            ] {
                let path = Candidate { leaf: 0, span };
                for first in [0, 3, 1999, 4096, 11997, u64::MAX - 4096] {
                    let range = path.samples(first, 4096, rate, fps);
                    for offset in 0..4096 {
                        let numerator =
                            (i128::from(first + offset) * 2 + 1) * i128::from(fps.numerator);
                        let denominator = i128::from(rate) * i128::from(fps.denominator) * 2;
                        let active = numerator >= i128::from(span.start) * denominator
                            && numerator <= i128::from(span.end) * denominator;
                        assert_eq!(
                            range.contains(&(offset as usize)),
                            active,
                            "{first} + {offset}, {span:?}, {rate}, {fps:?}"
                        );
                    }
                }
            }
        }
    }

    fn clip() -> Clip {
        Clip {
            id: Uuid::from_u128(1),
            name: "Index math".into(),
            range: FrameRange {
                start: 100,
                end: 148,
            },
            source: ClipSource::Composition {
                composition: Uuid::from_u128(2),
            },
            time_map: TimeMap {
                source_denominator: 1,
                points: vec![
                    TimePoint {
                        frame: 0,
                        source_tick: 24,
                    },
                    TimePoint {
                        frame: 10,
                        source_tick: 0,
                    },
                    TimePoint {
                        frame: 20,
                        source_tick: 48,
                    },
                    TimePoint {
                        frame: 30,
                        source_tick: 48,
                    },
                    TimePoint {
                        frame: 48,
                        source_tick: -12,
                    },
                ],
            },
            linked: None,
        }
    }

    #[test]
    fn inverse_bounds_never_omit_forward_reverse_boundary_or_nested_fractional_time() {
        let clip = clip();
        let child = vec![
            Span { start: 3, end: 9 },
            Span { start: 20, end: 24 },
            Span { start: 48, end: 48 },
        ];
        let mut builder = Builder::new(64);
        let intervals = builder.preimage(Some(child.clone()), &clip).unwrap();
        for tick in 0..=48 * 32 {
            let local = SourcePosition::new(tick, 32).unwrap();
            let source = clip.time_map.position_at(local).unwrap();
            for before in [false, true] {
                let active = clip.time_map.direction_validated(local, before).unwrap() != 0
                    && child.iter().any(|span| {
                        source.compare_tick(span.start).unwrap().is_ge()
                            && source.compare_tick(span.end).unwrap().is_le()
                    });
                if active {
                    let parent = SourcePosition::new(tick + 3200, 32).unwrap();
                    assert!(
                        intervals.iter().any(|span| parent
                            .compare_tick(span.start)
                            .unwrap()
                            .is_ge()
                            && parent.compare_tick(span.end).unwrap().is_le()),
                        "omitted {local:?} -> {source:?}"
                    );
                }
            }
        }
        let mut outer = clip.clone();
        outer.range = FrameRange { start: 0, end: 48 };
        outer.time_map.points = vec![
            TimePoint {
                frame: 0,
                source_tick: 148,
            },
            TimePoint {
                frame: 48,
                source_tick: 100,
            },
        ];
        let root = builder.preimage(Some(intervals), &outer).unwrap();
        for tick in 1..48 * 32 {
            let position = SourcePosition::new(tick, 32).unwrap();
            let inner = outer
                .time_map
                .position_at(position)
                .unwrap()
                .relative_to(100)
                .unwrap();
            let source = clip.time_map.position_at(inner).unwrap();
            if clip.time_map.direction_validated(inner, true).unwrap() != 0
                && child.iter().any(|span| {
                    source.compare_tick(span.start).unwrap().is_ge()
                        && source.compare_tick(span.end).unwrap().is_le()
                })
            {
                assert!(
                    root.iter()
                        .any(|span| position.compare_tick(span.start).unwrap().is_ge()
                            && position.compare_tick(span.end).unwrap().is_le())
                );
            }
        }
    }

    #[test]
    fn inverse_reduces_extreme_knots_and_enforces_compilation_budgets() {
        let mut clip = clip();
        clip.time_map.points = vec![
            TimePoint {
                frame: 0,
                source_tick: i64::MIN,
            },
            TimePoint {
                frame: 48,
                source_tick: i64::MAX,
            },
        ];
        let mut builder = Builder::new(64);
        let spans = builder
            .preimage(Some(vec![Span { start: -1, end: 1 }]), &clip)
            .unwrap();
        assert_eq!(
            spans,
            [Span {
                start: 123,
                end: 125
            }]
        );
        builder.remaining = 1;
        assert!(builder.preimage(None, &clip).is_err());
        assert!(Builder::new(0).preimage(None, &clip).is_err());
        assert!(
            Span::range(FrameRange {
                start: 0,
                end: u64::MAX
            })
            .is_err()
        );
    }

    #[test]
    fn balanced_lookup_skips_long_history_and_preserves_mix_order() {
        let mut index = Index::default();
        index
            .add(
                0,
                vec![Span {
                    start: 0,
                    end: 200000,
                }],
                20000,
            )
            .unwrap();
        for id in 1..16384 {
            index
                .add(
                    id,
                    vec![Span {
                        start: id as i64 * 10,
                        end: id as i64 * 10 + 2,
                    }],
                    20000,
                )
                .unwrap();
        }
        index.finish();
        let first = SourcePosition::new(163831, 1).unwrap();
        let (paths, visited) = index.query(first, first, 2).unwrap();
        assert_eq!(
            paths.iter().map(|path| path.leaf).collect::<Vec<_>>(),
            [0, 16383]
        );
        assert!(visited < 64, "inspected {visited} nodes");
        assert!(index.query(first, first, 1).is_err());
        let empty = SourcePosition::new(300000, 1).unwrap();
        assert_eq!(index.query(empty, empty, 1).unwrap(), (vec![], 1));
    }

    #[test]
    fn repeated_intervals_deduplicate_paths_and_bound_lookup_work() {
        let mut index = Index::default();
        index.add(2, vec![Span { start: 0, end: 8 }], 16).unwrap();
        index
            .add(
                0,
                vec![Span { start: 1, end: 2 }, Span { start: 4, end: 5 }],
                16,
            )
            .unwrap();
        index.add(1, vec![Span { start: 2, end: 7 }], 16).unwrap();
        index.finish();
        assert_eq!(
            index
                .query(
                    SourcePosition::new(0, 1).unwrap(),
                    SourcePosition::new(9, 1).unwrap(),
                    3
                )
                .unwrap()
                .0
                .iter()
                .map(|path| path.leaf)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(index.add(5, vec![Span { start: 0, end: 1 }], 4).is_err());
        let mut index = Index::default();
        index
            .add(
                0,
                (0..5000)
                    .map(|n| Span {
                        start: n * 2,
                        end: n * 2,
                    })
                    .collect(),
                5000,
            )
            .unwrap();
        index.finish();
        assert!(
            index
                .query(
                    SourcePosition::new(0, 1).unwrap(),
                    SourcePosition::new(10000, 1).unwrap(),
                    1
                )
                .is_err()
        );
    }
}
