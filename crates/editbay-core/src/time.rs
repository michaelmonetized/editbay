use crate::{Error, FrameRate, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimeBase {
    pub numerator: u32,
    pub denominator: u32,
}

impl TimeBase {
    /// Validate seconds per source tick.
    /// Takes this time base and returns an error for zero components.
    pub fn validate(self) -> Result<()> {
        if self.numerator == 0 || self.denominator == 0 {
            return Err(Error::Invalid(
                "time base components must be positive".into(),
            ));
        }
        Ok(())
    }

    /// Map an exact source tick position to a frame or sample boundary.
    /// `position` is measured in this source's ticks; `rate` is units per second.
    /// Returns the floor boundary, including negative preroll, without float rounding.
    pub fn boundary(self, position: SourcePosition, rate: FrameRate) -> Result<i64> {
        self.validate()?;
        rate.validate()?;
        let numerator = u128::from(position.numerator.unsigned_abs())
            .checked_mul(u128::from(self.numerator))
            .and_then(|v| v.checked_mul(u128::from(rate.numerator)))
            .ok_or_else(overflow)?;
        let denominator = u128::from(position.denominator)
            .checked_mul(u128::from(self.denominator))
            .and_then(|v| v.checked_mul(u128::from(rate.denominator)))
            .ok_or_else(overflow)?;
        if denominator == 0 {
            return Err(Error::Invalid("source position denominator is zero".into()));
        }
        let quotient = numerator / denominator;
        let boundary = if position.numerator < 0 {
            -i128::try_from(quotient + u128::from(!numerator.is_multiple_of(denominator)))
                .map_err(|_| overflow())?
        } else {
            i128::try_from(quotient).map_err(|_| overflow())?
        };
        i64::try_from(boundary).map_err(|_| overflow())
    }

    /// Convert source ticks to exact frame or sample units.
    /// `position` uses this time base and `rate` is units per second. Returns a
    /// reduced fraction, including negative preroll, or an explicit range error.
    pub fn at_rate(self, position: SourcePosition, rate: FrameRate) -> Result<SourcePosition> {
        self.validate()?;
        rate.validate()?;
        if position.denominator == 0 {
            return Err(Error::Invalid("source position denominator is zero".into()));
        }
        if position.numerator == 0 {
            return SourcePosition::new(0, 1);
        }
        if self.numerator == rate.denominator && self.denominator == rate.numerator {
            return SourcePosition::new(position.numerator, position.denominator);
        }
        let numerator = i128::from(position.numerator)
            .checked_mul(i128::from(self.numerator))
            .and_then(|value| value.checked_mul(i128::from(rate.numerator)))
            .ok_or_else(overflow)?;
        let denominator = u128::from(position.denominator)
            .checked_mul(u128::from(self.denominator))
            .and_then(|value| value.checked_mul(u128::from(rate.denominator)))
            .ok_or_else(overflow)?;
        let factor = divisor(numerator.unsigned_abs(), denominator);
        SourcePosition::from_fraction(
            numerator / i128::try_from(factor).map_err(|_| overflow())?,
            u64::try_from(denominator / factor).map_err(|_| overflow())?,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourcePosition {
    pub numerator: i64,
    pub denominator: u64,
}

impl SourcePosition {
    /// Construct a reduced exact time position.
    /// `numerator` and `denominator` specify source ticks or composition frames.
    /// Returns the canonical fraction, or an error for a zero denominator.
    pub fn new(numerator: i64, denominator: u64) -> Result<Self> {
        Self::from_fraction(i128::from(numerator), denominator)
    }
    /// Remove a source's presentation origin without rounding.
    /// `tick` is an integer origin in this position's units. Returns an exact offset.
    pub fn relative_to(self, tick: i64) -> Result<Self> {
        let numerator =
            i128::from(self.numerator) - i128::from(tick) * i128::from(self.denominator);
        Self::from_fraction(numerator, self.denominator)
    }
    /// Reduce an intermediate exact time fraction before narrowing it.
    /// `numerator` and `denominator` are signed ticks and a positive scale.
    /// Returns a representable canonical source position or an explicit error.
    pub(crate) fn from_fraction(numerator: i128, denominator: u64) -> Result<Self> {
        if denominator == 0 {
            return Err(Error::Invalid("source position denominator is zero".into()));
        }
        let a = divisor(numerator.unsigned_abs(), u128::from(denominator));
        let divisor = i128::try_from(a).map_err(|_| overflow())?;
        Ok(Self {
            numerator: i64::try_from(numerator / divisor).map_err(|_| overflow())?,
            denominator: denominator / u64::try_from(a).map_err(|_| overflow())?,
        })
    }

    /// Compare an exact tick position with an integer tick.
    /// `tick` is in the same source time base. Returns their exact ordering.
    pub fn compare_tick(self, tick: i64) -> Result<std::cmp::Ordering> {
        if self.denominator == 0 {
            return Err(Error::Invalid("source position denominator is zero".into()));
        }
        Ok(i128::from(self.numerator).cmp(&(i128::from(tick) * i128::from(self.denominator))))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimePoint {
    pub frame: u64,
    pub source_tick: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TimeMap {
    #[serde(
        default = "integer_source_unit",
        skip_serializing_if = "is_integer_source_unit"
    )]
    pub source_denominator: u32,
    pub points: Vec<TimePoint>,
}

/// Keep legacy maps in whole source ticks.
/// Takes no arguments and returns the default denominator, one.
fn integer_source_unit() -> u32 {
    1
}

/// Preserve legacy serialized maps when no fractional source units are needed.
/// `value` is a source denominator; returns true for whole source ticks.
fn is_integer_source_unit(value: &u32) -> bool {
    *value == 1
}

impl TimeMap {
    /// Inspect the direction of the segment owning a local frame boundary.
    /// `frame` is clip-local time. Returns -1, 0 or 1 for reverse, freeze or forward.
    pub fn direction_at(&self, frame: u64) -> Result<i8> {
        let duration = self
            .points
            .last()
            .ok_or_else(|| Error::Invalid("empty time map".into()))?
            .frame;
        self.validate(duration)?;
        let index = self.segment(u128::from(frame), 1, false)?;
        self.direction(index)
    }
    /// Validate a clip-local piecewise linear source mapping.
    /// `duration` is the exclusive local frame end. Returns an error for gaps in
    /// the boundary definition, duplicate points or unrepresentable interpolation.
    pub fn validate(&self, duration: u64) -> Result<()> {
        if duration == 0
            || self.source_denominator == 0
            || self.points.len() < 2
            || self.points.len() > 65_536
            || self.points[0].frame != 0
            || self
                .points
                .last()
                .is_none_or(|point| point.frame != duration)
            || self.points.windows(2).any(|p| p[0].frame >= p[1].frame)
        {
            return Err(Error::Invalid(
                "time map needs increasing points at 0 and duration".into(),
            ));
        }
        Ok(())
    }

    /// Evaluate forward, reverse, freeze or variable-speed source time.
    /// `frame` is a clip-local boundary, including the final exclusive boundary.
    /// Returns a reduced exact tick fraction; no source frame rounding occurs here.
    pub fn position(&self, frame: u64) -> Result<SourcePosition> {
        let duration = self
            .points
            .last()
            .ok_or_else(|| Error::Invalid("empty time map".into()))?
            .frame;
        self.validate(duration)?;
        self.position_fraction(u128::from(frame), 1)
    }

    /// Evaluate an exact fractional clip boundary without rounding.
    /// `position` is measured in local composition frames, including the end.
    /// Returns a reduced source-tick fraction after validating the time map.
    pub fn position_at(&self, position: SourcePosition) -> Result<SourcePosition> {
        let duration = self
            .points
            .last()
            .ok_or_else(|| Error::Invalid("empty time map".into()))?
            .frame;
        self.validate(duration)?;
        self.position_validated(position)
    }

    /// Select direction after validating this map's ordered points.
    /// `position` is local frame time and `before` selects the preceding segment.
    /// Returns reverse, freeze or forward direction without rescanning the map.
    pub(crate) fn direction_validated(&self, position: SourcePosition, before: bool) -> Result<i8> {
        let numerator = u128::try_from(position.numerator)
            .map_err(|_| Error::Invalid("frame is outside the time map".into()))?;
        let index = self.segment(numerator, position.denominator, before)?;
        self.direction(index)
    }

    /// Inspect the local source-tick slope for resampling.
    /// `position` selects local frames and `before` owns an exact cut boundary.
    /// Returns signed ticks per frame; exact temporal positions remain rational.
    pub(crate) fn slope_validated(&self, position: SourcePosition, before: bool) -> Result<f64> {
        let numerator = u128::try_from(position.numerator)
            .map_err(|_| Error::Invalid("frame is outside the time map".into()))?;
        let index = self.segment(numerator, position.denominator, before)?;
        let a = self.points[index];
        let b = self.points[index + 1];
        Ok(
            (i128::from(b.source_tick) - i128::from(a.source_tick)) as f64
                / (b.frame - a.frame) as f64
                / f64::from(self.source_denominator),
        )
    }

    /// Inspect the slope sign of a retained segment.
    /// `index` selects consecutive validated points. Returns -1, 0 or 1.
    fn direction(&self, index: usize) -> Result<i8> {
        Ok(
            match self.points[index + 1]
                .source_tick
                .cmp(&self.points[index].source_tick)
            {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            },
        )
    }

    /// Locate the owning time-map segment with exact integer comparisons.
    /// `numerator`, `denominator` and `before` select time and boundary ownership.
    /// Returns the segment index, rejecting positions outside the retained map.
    fn segment(&self, numerator: u128, denominator: u64, before: bool) -> Result<usize> {
        let duration = self
            .points
            .last()
            .ok_or_else(|| Error::Invalid("empty time map".into()))?
            .frame;
        if self.points.len() < 2
            || denominator == 0
            || numerator > u128::from(duration) * u128::from(denominator)
        {
            return Err(Error::Invalid("frame is outside the time map".into()));
        }
        Ok(self
            .points
            .partition_point(|point| {
                let tick = u128::from(point.frame) * u128::from(denominator);
                if before {
                    tick < numerator
                } else {
                    tick <= numerator
                }
            })
            .saturating_sub(1)
            .min(self.points.len() - 2))
    }

    /// Map exact local time after ordered-point validation.
    /// `position` selects a fractional frame boundary. Returns reduced source time.
    pub(crate) fn position_validated(&self, position: SourcePosition) -> Result<SourcePosition> {
        let numerator = u128::try_from(position.numerator)
            .map_err(|_| Error::Invalid("frame is outside the time map".into()))?;
        self.position_fraction(numerator, position.denominator)
    }

    /// Interpolate one rational local boundary without early rounding.
    /// `numerator` and `denominator` specify nonnegative local frames.
    /// Returns reduced source ticks using cancellation before multiplication.
    fn position_fraction(&self, numerator: u128, denominator: u64) -> Result<SourcePosition> {
        let raw = self.position_numerators(numerator, denominator)?;
        let factor = divisor(
            raw.numerator.unsigned_abs().into(),
            self.source_denominator.into(),
        );
        SourcePosition::from_fraction(
            i128::from(raw.numerator) / factor as i128,
            raw.denominator
                .checked_mul(u64::from(self.source_denominator) / factor as u64)
                .ok_or_else(overflow)?,
        )
    }

    /// Interpolate stored numerators before applying the source unit.
    /// `numerator` and `denominator` select local frames; returns exact raw units.
    fn position_numerators(&self, numerator: u128, denominator: u64) -> Result<SourcePosition> {
        let index = self.segment(numerator, denominator, false)?;
        let a = self.points[index];
        let b = self.points[index + 1];
        let span = b.frame - a.frame;
        let offset = numerator - u128::from(a.frame) * u128::from(denominator);
        if denominator == 1 {
            let remaining = u128::from(span) - offset;
            let numerator = i128::from(a.source_tick)
                .checked_mul(remaining as i128)
                .and_then(|start| {
                    i128::from(b.source_tick)
                        .checked_mul(offset as i128)
                        .and_then(|end| start.checked_add(end))
                })
                .ok_or_else(overflow)?;
            return SourcePosition::from_fraction(numerator, span);
        }
        if let Some(scale) = span.checked_mul(denominator)
            && let Ok(offset) = i128::try_from(offset)
            && let Some(value) = (i128::from(b.source_tick) - i128::from(a.source_tick))
                .checked_mul(offset)
                .and_then(|delta| {
                    i128::from(a.source_tick)
                        .checked_mul(i128::from(scale))
                        .and_then(|start| start.checked_add(delta))
                })
        {
            return SourcePosition::from_fraction(value, scale);
        }
        let offset = SourcePosition::from_fraction(
            i128::try_from(offset).map_err(|_| overflow())?,
            denominator,
        )?;
        let mut delta = i128::from(b.source_tick) - i128::from(a.source_tick);
        let factor = divisor(delta.unsigned_abs(), u128::from(span)) as u64;
        delta /= i128::from(factor);
        let span = span / factor;
        let factor = divisor(offset.numerator.unsigned_abs().into(), span.into()) as u64;
        let offset_numerator = i128::from(offset.numerator) / i128::from(factor);
        let span = span / factor;
        let factor = divisor(delta.unsigned_abs(), offset.denominator.into()) as u64;
        delta /= i128::from(factor);
        let denominator = span
            .checked_mul(offset.denominator / factor)
            .ok_or_else(overflow)?;
        let numerator = i128::from(a.source_tick)
            .checked_mul(i128::from(denominator))
            .and_then(|start| {
                delta
                    .checked_mul(offset_numerator)
                    .and_then(|step| start.checked_add(step))
            })
            .ok_or_else(overflow)?;
        SourcePosition::from_fraction(numerator, denominator)
    }
}

/// Find a shared integer factor.
/// `a` and `b` are magnitudes. Returns their greatest common divisor.
fn divisor(mut a: u128, mut b: u128) -> u128 {
    if let (Ok(mut small_a), Ok(mut small_b)) = (u64::try_from(a), u64::try_from(b)) {
        while small_b != 0 {
            (small_a, small_b) = (small_b, small_a % small_b);
        }
        return u128::from(small_a);
    }
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn overflow() -> Error {
    Error::Invalid("exact time conversion exceeds its integer range".into())
}
