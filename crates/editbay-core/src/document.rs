use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

pub const PROJECT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRate {
    pub numerator: u32,
    pub denominator: u32,
}

impl FrameRate {
    pub fn new(numerator: u32, denominator: u32) -> Result<Self> {
        let rate = Self {
            numerator,
            denominator,
        };
        rate.validate()?;
        Ok(rate)
    }

    fn validate(self) -> Result<()> {
        if self.numerator == 0 || self.denominator == 0 {
            return Err(Error::Invalid(
                "frame rate must have positive numerator and denominator".into(),
            ));
        }
        Ok(())
    }

    /// Convert a nonnegative frame boundary to nanoseconds without float drift.
    pub fn frame_nanoseconds(self, frame: u64) -> Result<u128> {
        self.validate()?;
        Ok(
            u128::from(frame) * u128::from(self.denominator) * 1_000_000_000
                / u128::from(self.numerator),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sequence {
    pub id: Uuid,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema: u32,
    pub id: Uuid,
    pub revision: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_from: Option<Uuid>,
    pub sequences: Vec<Sequence>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let project = Self {
            schema: PROJECT_SCHEMA,
            id: Uuid::new_v4(),
            revision: 0,
            name: name.into(),
            recovered_from: None,
            sequences: vec![Sequence {
                id: Uuid::new_v4(),
                name: "Sequence 1".into(),
                width: 1920,
                height: 1080,
                frame_rate: FrameRate {
                    numerator: 24,
                    denominator: 1,
                },
            }],
        };
        project.validate()?;
        Ok(project)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != PROJECT_SCHEMA {
            return Err(Error::Schema(self.schema));
        }
        if self.id.is_nil() || self.name.trim().is_empty() {
            return Err(Error::Invalid(
                "project needs an identity and a nonempty name".into(),
            ));
        }
        if self
            .recovered_from
            .is_some_and(|id| id.is_nil() || id == self.id)
        {
            return Err(Error::Invalid("invalid recovery origin".into()));
        }
        if self.sequences.is_empty() {
            return Err(Error::Invalid("project needs at least one sequence".into()));
        }
        let mut identities = HashSet::from([self.id]);
        for sequence in &self.sequences {
            if sequence.id.is_nil() || !identities.insert(sequence.id) {
                return Err(Error::Invalid(
                    "sequence identities must be nonempty and unique".into(),
                ));
            }
            if sequence.name.trim().is_empty() || sequence.width == 0 || sequence.height == 0 {
                return Err(Error::Invalid(
                    "sequence needs a name and positive dimensions".into(),
                ));
            }
            sequence.frame_rate.validate()?;
        }
        Ok(())
    }

    pub fn rename(&mut self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::Invalid("project name cannot be empty".into()));
        }
        if name != self.name {
            let revision = self
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("revision counter overflow".into()))?;
            self.name = name;
            self.revision = revision;
        }
        Ok(())
    }
}
