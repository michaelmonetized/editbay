use crate::{Error, Project, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const HISTORY_LIMIT: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DocumentVersion {
    pub project_id: Uuid,
    pub revision: u64,
}

impl DocumentVersion {
    /// Capture document ownership for a command.
    /// `project` supplies identity/revision. Returns the version a caller must retain.
    pub fn of(project: &Project) -> Self {
        Self {
            project_id: project.id,
            revision: project.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentCommand {
    RenameProject { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandReceipt {
    pub before: DocumentVersion,
    pub after: DocumentVersion,
    pub changed: bool,
}

#[derive(Debug, Clone)]
struct NameChange {
    before: String,
    after: String,
    label: String,
}

#[derive(Debug, Clone)]
pub struct DocumentEditor {
    project: Project,
    undo: Vec<NameChange>,
    redo: Vec<NameChange>,
}

impl DocumentEditor {
    /// Own a validated document and its local command history.
    /// `project` is the loaded or untitled document. Returns empty bounded history;
    /// persisted snapshots retain document state, not this in-memory history.
    pub fn new(project: Project) -> Result<Self> {
        project.validate()?;
        Ok(Self {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    /// Inspect the current document without bypassing its commands.
    /// Takes no arguments and returns an immutable view owned by this editor.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Inspect the next available undo and redo actions.
    /// Takes no arguments and returns optional group labels, without changing history.
    pub fn history(&self) -> (Option<&str>, Option<&str>) {
        (
            self.undo.last().map(|h| h.label.as_str()),
            self.redo.last().map(|h| h.label.as_str()),
        )
    }

    /// Apply a complete reversible command group.
    /// `expected` owns the current identity/revision; `label` names this undo step;
    /// `commands` contains 1..64 implemented commands. Returns one revision change
    /// for the whole validated group. Rejected groups leave document/history untouched.
    pub fn apply(
        &mut self,
        expected: DocumentVersion,
        label: String,
        commands: &[DocumentCommand],
    ) -> Result<CommandReceipt> {
        self.check(expected)?;
        if label.trim().is_empty()
            || label.len() > 256
            || commands.is_empty()
            || commands.len() > 64
        {
            return Err(Error::Invalid(
                "command group requires a 1..256 byte label and 1..64 commands".into(),
            ));
        }
        let mut next = self.project.clone();
        for command in commands {
            match command {
                DocumentCommand::RenameProject { name } => next.name = name.clone(),
            }
            next.validate()?;
        }
        let changed = next.name != self.project.name;
        if changed {
            next.revision = self.next_revision()?;
            let change = NameChange {
                before: self.project.name.clone(),
                after: next.name.clone(),
                label,
            };
            self.undo.push(change);
            if self.undo.len() > HISTORY_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.project = next;
        }
        Ok(self.receipt(expected, changed))
    }

    /// Undo the most recent command group with a new revision.
    /// `expected` must match this editor. Returns the changed ownership; stale jobs
    /// cannot become current again because revisions never move backwards.
    pub fn undo(&mut self, expected: DocumentVersion) -> Result<CommandReceipt> {
        self.check(expected)?;
        let change = self
            .undo
            .last()
            .ok_or_else(|| Error::Invalid("no command to undo".into()))?;
        let mut next = self.project.clone();
        next.name = change.before.clone();
        next.revision = self.next_revision()?;
        next.validate()?;
        let change = self.undo.pop().unwrap();
        self.redo.push(change);
        self.project = next;
        Ok(self.receipt(expected, true))
    }

    /// Reapply the most recently undone command group with a new revision.
    /// `expected` must match this editor. Returns updated ownership after validation.
    pub fn redo(&mut self, expected: DocumentVersion) -> Result<CommandReceipt> {
        self.check(expected)?;
        let change = self
            .redo
            .last()
            .ok_or_else(|| Error::Invalid("no command to redo".into()))?;
        let mut next = self.project.clone();
        next.name = change.after.clone();
        next.revision = self.next_revision()?;
        next.validate()?;
        let change = self.redo.pop().unwrap();
        self.undo.push(change);
        self.project = next;
        Ok(self.receipt(expected, true))
    }

    fn check(&self, expected: DocumentVersion) -> Result<()> {
        if expected.project_id != self.project.id {
            return Err(Error::Invalid("command belongs to another project".into()));
        }
        if expected.revision != self.project.revision {
            return Err(Error::StaleCommand {
                expected: expected.revision,
                current: self.project.revision,
            });
        }
        Ok(())
    }
    fn next_revision(&self) -> Result<u64> {
        self.project
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("revision counter overflow".into()))
    }
    fn receipt(&self, before: DocumentVersion, changed: bool) -> CommandReceipt {
        CommandReceipt {
            before,
            after: DocumentVersion::of(&self.project),
            changed,
        }
    }
}
