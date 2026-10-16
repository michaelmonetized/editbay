use crate::{DocumentCommand, Error, Project, Result, StreamFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Original bounded RGB ICC bytes retained with their content identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct IccProfile {
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// Assign or clear one retained source profile through the shared document command.
/// `project`, `source`, `stream` and `profile` select the captured video interpretation.
/// Returns a complete SetSource command; history, validation and persistence stay shared.
pub fn source_icc_command(
    project: &Project,
    source: uuid::Uuid,
    stream: u32,
    profile: Option<IccProfile>,
) -> Result<DocumentCommand> {
    if let Some(profile) = &profile {
        profile.validate()?;
    }
    let mut source = project
        .sources
        .iter()
        .find(|item| item.id == source)
        .ok_or_else(|| Error::Invalid("ICC source is absent".into()))?
        .clone();
    let stream = source
        .streams
        .iter_mut()
        .find(|item| item.index == stream)
        .ok_or_else(|| Error::Invalid("ICC video stream is absent".into()))?;
    if !matches!(stream.format, StreamFormat::Video { .. }) {
        return Err(Error::Invalid(
            "ICC profile cannot interpret a sound stream".into(),
        ));
    }
    stream.icc_profile = profile;
    Ok(DocumentCommand::SetSource { source })
}

impl IccProfile {
    /// Retain an original RGB profile without rewriting its tags or curves.
    /// `bytes` contains the entire profile; returns validated data and SHA-256.
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        let profile = Self {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Check profile framing, RGB encoding and exact captured bytes.
    /// Takes this profile; returns an error before native parsing for oversized,
    /// truncated, non-RGB or modified data. Native color parsing remains required.
    pub fn validate(&self) -> Result<()> {
        if !(128..=1024 * 1024).contains(&self.bytes.len())
            || u32::from_be_bytes(self.bytes[..4].try_into().unwrap()) as usize != self.bytes.len()
            || &self.bytes[36..40] != b"acsp"
            || &self.bytes[16..20] != b"RGB "
            || self.sha256 != format!("{:x}", Sha256::digest(&self.bytes))
        {
            return Err(Error::Invalid(
                "ICC profile needs complete unmodified RGB data within 1 MiB".into(),
            ));
        }
        Ok(())
    }
}
