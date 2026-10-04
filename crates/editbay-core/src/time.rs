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
        let numerator = i128::from(position.numerator)
            .checked_mul(i128::from(self.numerator))
            .and_then(|v| v.checked_mul(i128::from(rate.numerator)))
            .ok_or_else(overflow)?;
        let denominator = i128::from(position.denominator)
            .checked_mul(i128::from(self.denominator))
            .and_then(|v| v.checked_mul(i128::from(rate.denominator)))
            .ok_or_else(overflow)?;
        if denominator == 0 {
            return Err(Error::Invalid("source position denominator is zero".into()));
        }
        i64::try_from(numerator.div_euclid(denominator)).map_err(|_| overflow())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourcePosition {
    pub numerator: i64,
    pub denominator: u64,
}

impl SourcePosition {
    /// Remove a source's presentation origin without rounding.
    /// `tick` is an integer origin in this position's units. Returns an exact offset.
    pub fn relative_to(self, tick: i64) -> Result<Self> {
        let numerator =
            i128::from(self.numerator) - i128::from(tick) * i128::from(self.denominator);
        Self::from_fraction(numerator, self.denominator)
    }
    pub(crate) fn from_fraction(numerator: i128, denominator: u64) -> Result<Self> {
        if denominator == 0 {
            return Err(Error::Invalid("source position denominator is zero".into()));
        }
        let mut a = numerator.unsigned_abs();
        let mut b = u128::from(denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
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
    pub points: Vec<TimePoint>,
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
        if frame > duration {
            return Err(Error::Invalid("frame is outside the time map".into()));
        }
        let index = self
            .points
            .partition_point(|point| point.frame <= frame)
            .saturating_sub(1)
            .min(self.points.len() - 2);
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
    /// Validate a clip-local piecewise linear source mapping.
    /// `duration` is the exclusive local frame end. Returns an error for gaps in
    /// the boundary definition, duplicate points or unrepresentable interpolation.
    pub fn validate(&self, duration: u64) -> Result<()> {
        if duration == 0
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
        if frame > duration {
            return Err(Error::Invalid("frame is outside the time map".into()));
        }
        let index = self
            .points
            .partition_point(|point| point.frame <= frame)
            .saturating_sub(1);
        if index == self.points.len() - 1 {
            return Ok(SourcePosition {
                numerator: self.points[index].source_tick,
                denominator: 1,
            });
        }
        let a = self.points[index];
        let b = self.points[index + 1];
        let span = b.frame - a.frame;
        let offset = frame - a.frame;
        let numerator = i128::from(a.source_tick)
            .checked_mul(i128::from(span - offset))
            .and_then(|v| {
                i128::from(b.source_tick)
                    .checked_mul(i128::from(offset))
                    .and_then(|step| v.checked_add(step))
            })
            .ok_or_else(overflow)?;
        SourcePosition::from_fraction(numerator, span)
    }
}

fn overflow() -> Error {
    Error::Invalid("exact time conversion exceeds its integer range".into())
}
