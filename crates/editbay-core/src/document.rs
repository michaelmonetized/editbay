use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

pub const PROJECT_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
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

    pub(crate) fn validate(self) -> Result<()> {
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
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Sequence {
    pub id: Uuid,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema: u32,
    pub id: Uuid,
    pub revision: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_from: Option<Uuid>,
    pub sequences: Vec<Sequence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<crate::AssetReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<crate::MediaSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compositions: Vec<crate::Composition>,
    #[serde(default, skip_serializing_if = "crate::ColorConfiguration::is_default")]
    pub color: crate::ColorConfiguration,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let project = Self {
            schema: PROJECT_SCHEMA,
            id: Uuid::new_v4(),
            revision: 0,
            name: name.into(),
            recovered_from: None,
            assets: Vec::new(),
            sources: Vec::new(),
            compositions: Vec::new(),
            color: crate::ColorConfiguration::default(),
            sequences: vec![Sequence {
                id: Uuid::new_v4(),
                name: "Sequence 1".into(),
                width: 1920,
                height: 1080,
                frame_rate: FrameRate {
                    numerator: 24,
                    denominator: 1,
                },
                composition: None,
            }],
        };
        project.validate()?;
        Ok(project)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != PROJECT_SCHEMA {
            return Err(Error::Schema(self.schema));
        }
        crate::composition::named(&self.name)?;
        if self.id.is_nil() {
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
            crate::composition::named(&sequence.name)?;
            if sequence.width == 0 || sequence.height == 0 {
                return Err(Error::Invalid(
                    "sequence dimensions must be positive".into(),
                ));
            }
            sequence.frame_rate.validate()?;
        }
        crate::composition::validate_model(self, &mut identities)?;
        Ok(())
    }

    /// Upgrade a supported older representation without writing its source.
    /// Takes the parsed document and returns schema 2 with its identity, revision
    /// and sequence profiles retained. Recovery callers verify integrity first.
    pub fn migrate(mut self) -> Result<Self> {
        if self.schema == 1 {
            if !self.assets.is_empty()
                || !self.sources.is_empty()
                || !self.compositions.is_empty()
                || self
                    .sequences
                    .iter()
                    .any(|sequence| sequence.composition.is_some())
                || !self.color.is_default()
            {
                return Err(Error::Invalid(
                    "schema 1 cannot contain schema 2 media/composition fields".into(),
                ));
            }
            self.schema = PROJECT_SCHEMA;
        }
        self.validate()?;
        Ok(self)
    }

    pub fn rename(&mut self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        crate::composition::named(&name)?;
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
