use crate::{DeliveryRequest, Phase, Progress, Receipt, Result, engine::Session};
use editbay_core::{DocumentVersion, Project};
use editbay_media::native_job::JobChannel;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Owner {
    pub job: Uuid,
    pub version: DocumentVersion,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub owner: Owner,
    pub serial: u64,
    pub operation: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Operation {
    Begin {
        project: Box<Project>,
        request: DeliveryRequest,
    },
    Step,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    pub owner: Owner,
    pub serial: u64,
    pub outcome: Outcome,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Outcome {
    Progress {
        progress: Progress,
        receipt: Option<Box<Receipt>>,
    },
    Failed {
        message: String,
    },
}

/// Serve one captured delivery through the inherited supervised job socket.
/// Takes no arguments. Returns at EOF, failure or verified completion; the child
/// can only write its received temporary descriptor and cannot publish a path.
pub fn serve() -> Result<()> {
    let mut channel = JobChannel::inherited()?;
    let mut session = None;
    let mut owner = None;
    let mut serial = 0;
    while let Some((request, file)) = channel.receive::<Request>()? {
        let operation = || -> Result<()> {
            if request.serial != serial || owner.is_some_and(|owner| owner != request.owner) {
                return Err("Delivery operation is stale or belongs to another owner".into());
            }
            match request.operation {
                Operation::Begin {
                    project,
                    request: delivery,
                } => {
                    if session.is_some()
                        || serial != 0
                        || DocumentVersion::of(&project) != request.owner.version
                    {
                        return Err("Delivery binding does not own its document".into());
                    }
                    session = Some(Session::new(
                        Arc::new(*project),
                        delivery,
                        file.ok_or("Delivery has no private output descriptor")?,
                    )?);
                    owner = Some(request.owner);
                }
                Operation::Step => {
                    if file.is_some() {
                        return Err("Delivery step included a foreign descriptor".into());
                    }
                    session.as_mut().ok_or("Delivery is not bound")?.step()?;
                }
            }
            serial = serial.checked_add(1).ok_or("Delivery serial exhausted")?;
            Ok(())
        };
        let outcome = match operation() {
            Ok(()) => {
                let session = session.as_ref().ok_or("Delivery session is absent")?;
                Outcome::Progress {
                    progress: session.progress(),
                    receipt: session.receipt().cloned().map(Box::new),
                }
            }
            Err(error) => Outcome::Failed {
                message: error.to_string(),
            },
        };
        let terminal = matches!(&outcome, Outcome::Failed { .. })
            || matches!(&outcome, Outcome::Progress { progress, .. } if progress.phase == Phase::Complete);
        channel.send(&Response {
            owner: request.owner,
            serial: request.serial,
            outcome,
        })?;
        if terminal {
            break;
        }
    }
    Ok(())
}
