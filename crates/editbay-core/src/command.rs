use crate::{
    AssetReference, ClipSource, ColorConfiguration, Composition, Error, MediaSource, Project,
    Result, Sequence,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

const HISTORY_LIMIT: usize = 128;
const HISTORY_BYTES: usize = 16 * 1024 * 1024;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentCommand {
    RenameProject { name: String },
    SetSequence { sequence: Sequence },
    SetPrimarySequence { id: Uuid },
    RemoveSequence { id: Uuid },
    SetAsset { asset: AssetReference },
    RemoveAsset { id: Uuid },
    SetSource { source: MediaSource },
    RemoveSource { id: Uuid },
    SetComposition { composition: Composition },
    RemoveComposition { id: Uuid },
    SetColor { configuration: ColorConfiguration },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CommandGroup {
    pub expected: DocumentVersion,
    pub label: String,
    pub commands: Vec<DocumentCommand>,
}

impl DocumentCommand {
    /// List implemented command names for automation capability discovery.
    /// Takes no arguments and returns stable wire names accepted by the core.
    pub fn capabilities() -> &'static [&'static str] {
        &[
            "rename_project",
            "set_sequence",
            "set_primary_sequence",
            "remove_sequence",
            "set_asset",
            "remove_asset",
            "set_source",
            "remove_source",
            "set_composition",
            "remove_composition",
            "set_color",
        ]
    }

    fn apply(&self, project: &mut Project) -> Result<()> {
        match self {
            Self::RenameProject { name } => project.name = name.clone(),
            Self::SetSequence { sequence } => set(&mut project.sequences, sequence.clone()),
            Self::SetPrimarySequence { id } => {
                let index = project
                    .sequences
                    .iter()
                    .position(|sequence| sequence.id == *id)
                    .ok_or_else(|| Error::Invalid("primary sequence does not exist".into()))?;
                let sequence = project.sequences.remove(index);
                project.sequences.insert(0, sequence);
            }
            Self::RemoveSequence { id } => remove(&mut project.sequences, *id)?,
            Self::SetAsset { asset } => set(&mut project.assets, asset.clone()),
            Self::RemoveAsset { id } => remove(&mut project.assets, *id)?,
            Self::SetSource { source } => set(&mut project.sources, source.clone()),
            Self::RemoveSource { id } => remove(&mut project.sources, *id)?,
            Self::SetComposition { composition } => {
                set(&mut project.compositions, composition.clone())
            }
            Self::RemoveComposition { id } => remove(&mut project.compositions, *id)?,
            Self::SetColor { configuration } => project.color = *configuration,
        }
        project.validate()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChangeImpact {
    pub entities: Vec<Uuid>,
    pub compositions: Vec<Uuid>,
    pub color_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandReceipt {
    pub before: DocumentVersion,
    pub after: DocumentVersion,
    pub changed: bool,
    pub impact: ChangeImpact,
}

trait Identified {
    fn id(&self) -> Uuid;
}
impl Identified for Sequence {
    fn id(&self) -> Uuid {
        self.id
    }
}
impl Identified for AssetReference {
    fn id(&self) -> Uuid {
        self.id
    }
}
impl Identified for MediaSource {
    fn id(&self) -> Uuid {
        self.id
    }
}
impl Identified for Composition {
    fn id(&self) -> Uuid {
        self.id
    }
}

fn set<T: Identified>(entities: &mut Vec<T>, entity: T) {
    if let Some(current) = entities
        .iter_mut()
        .find(|current| current.id() == entity.id())
    {
        *current = entity;
    } else {
        entities.push(entity);
    }
}

fn remove<T: Identified>(entities: &mut Vec<T>, id: Uuid) -> Result<()> {
    let index = entities
        .iter()
        .position(|entity| entity.id() == id)
        .ok_or_else(|| Error::Invalid("command entity is absent".into()))?;
    entities.remove(index);
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct Delta<T> {
    id: Uuid,
    before: Option<(usize, T)>,
    after: Option<(usize, T)>,
}

fn differences<T: Identified + Clone + PartialEq>(before: &[T], after: &[T]) -> Vec<Delta<T>> {
    let old: HashMap<_, _> = before
        .iter()
        .enumerate()
        .map(|(index, entity)| (entity.id(), (index, entity)))
        .collect();
    let new: HashMap<_, _> = after
        .iter()
        .enumerate()
        .map(|(index, entity)| (entity.id(), (index, entity)))
        .collect();
    let ids: BTreeSet<_> = old.keys().chain(new.keys()).copied().collect();
    ids.into_iter()
        .filter_map(|id| {
            let a = old.get(&id);
            let b = new.get(&id);
            if a == b {
                return None;
            }
            Some(Delta {
                id,
                before: a.map(|(index, entity)| (*index, (*entity).clone())),
                after: b.map(|(index, entity)| (*index, (*entity).clone())),
            })
        })
        .collect()
}

fn restore<T: Identified + Clone>(entities: &mut Vec<T>, deltas: &[Delta<T>], forward: bool) {
    let ids: HashSet<_> = deltas.iter().map(|delta| delta.id).collect();
    entities.retain(|entity| !ids.contains(&entity.id()));
    let mut insertions: Vec<_> = deltas
        .iter()
        .filter_map(|delta| {
            if forward {
                delta.after.as_ref()
            } else {
                delta.before.as_ref()
            }
        })
        .collect();
    insertions.sort_by_key(|(index, _)| *index);
    for (index, entity) in insertions {
        entities.insert(*index, entity.clone());
    }
}

#[derive(Debug, Clone, Serialize)]
struct GroupChange {
    name: Option<(String, String)>,
    color: Option<(ColorConfiguration, ColorConfiguration)>,
    sequences: Vec<Delta<Sequence>>,
    assets: Vec<Delta<AssetReference>>,
    sources: Vec<Delta<MediaSource>>,
    compositions: Vec<Delta<Composition>>,
    impact: ChangeImpact,
    label: String,
    bytes: usize,
}

impl GroupChange {
    fn between(before: &Project, after: &Project, label: String) -> Result<Self> {
        let mut group = Self {
            name: (before.name != after.name).then(|| (before.name.clone(), after.name.clone())),
            color: (before.color != after.color).then_some((before.color, after.color)),
            sequences: differences(&before.sequences, &after.sequences),
            assets: differences(&before.assets, &after.assets),
            sources: differences(&before.sources, &after.sources),
            compositions: differences(&before.compositions, &after.compositions),
            impact: ChangeImpact::default(),
            label,
            bytes: 0,
        };
        group.impact = group.impact_for(before, after);
        group.bytes = serde_json::to_vec(&group)?.len();
        if group.bytes > HISTORY_BYTES {
            return Err(Error::Invalid(
                "undo group exceeds the 16 MiB history budget".into(),
            ));
        }
        Ok(group)
    }

    fn restore(&self, project: &mut Project, forward: bool) {
        if let Some((before, after)) = &self.name {
            project.name = if forward { after } else { before }.clone();
        }
        if let Some((before, after)) = self.color {
            project.color = if forward { after } else { before };
        }
        restore(&mut project.sequences, &self.sequences, forward);
        restore(&mut project.assets, &self.assets, forward);
        restore(&mut project.sources, &self.sources, forward);
        restore(&mut project.compositions, &self.compositions, forward);
    }

    fn impact_for(&self, before: &Project, after: &Project) -> ChangeImpact {
        let assets: HashSet<_> = self.assets.iter().map(|delta| delta.id).collect();
        let mut sources: HashSet<_> = self.sources.iter().map(|delta| delta.id).collect();
        let all_compositions: Vec<_> = before
            .compositions
            .iter()
            .chain(&after.compositions)
            .collect();
        for source in before.sources.iter().chain(&after.sources) {
            if assets.contains(&source.asset) {
                sources.insert(source.id);
            }
        }
        let mut affected: BTreeSet<_> = self.compositions.iter().map(|delta| delta.id).collect();
        for composition in &all_compositions {
            if self.color.is_some()
                || composition.tracks.iter().flat_map(|track| &track.clips).any(|clip| matches!(clip.source, ClipSource::Media { source, .. } if sources.contains(&source)))
                || composition.nodes.iter().any(|node| matches!(node.operation, crate::NodeOperation::MaskAsset { asset } | crate::NodeOperation::Text { font: asset, .. } if assets.contains(&asset))) {
                affected.insert(composition.id);
            }
        }
        let mut parents: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for composition in &all_compositions {
            for clip in composition.tracks.iter().flat_map(|track| &track.clips) {
                if let ClipSource::Composition { composition: child } = clip.source {
                    parents.entry(child).or_default().push(composition.id);
                }
            }
        }
        let mut pending: Vec<_> = affected.iter().copied().collect();
        while let Some(child) = pending.pop() {
            for parent in parents.get(&child).into_iter().flatten() {
                if affected.insert(*parent) {
                    pending.push(*parent);
                }
            }
        }
        let mut entities: BTreeSet<_> = assets
            .into_iter()
            .chain(sources)
            .chain(self.sequences.iter().map(|delta| delta.id))
            .chain(self.compositions.iter().map(|delta| delta.id))
            .collect();
        if self.name.is_some() || self.color.is_some() {
            entities.insert(before.id);
        }
        ChangeImpact {
            entities: entities.into_iter().collect(),
            compositions: affected.into_iter().collect(),
            color_changed: self.color.is_some(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentEditor {
    project: Arc<Project>,
    undo: Vec<Arc<GroupChange>>,
    redo: Vec<Arc<GroupChange>>,
}

impl DocumentEditor {
    /// Own a validated document and bounded local command history.
    /// `project` is a loaded or untitled current-schema document. Returns an editor;
    /// persisted snapshots retain state, while undo history stays in this session.
    pub fn new(project: Project) -> Result<Self> {
        project.validate()?;
        Ok(Self {
            project: Arc::new(project),
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    /// Inspect the current document without bypassing commands.
    /// Takes no arguments and returns this editor's immutable document.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Capture immutable document state without cloning media or graph data.
    /// Takes no arguments and returns shared state safe to retain in frames/jobs.
    /// Later commands replace this state, preserving the captured revision.
    pub fn snapshot(&self) -> Arc<Project> {
        self.project.clone()
    }

    /// Inspect the next undo and redo actions.
    /// Takes no arguments and returns optional labels without changing history.
    pub fn history(&self) -> (Option<&str>, Option<&str>) {
        (
            self.undo.last().map(|group| group.label.as_str()),
            self.redo.last().map(|group| group.label.as_str()),
        )
    }

    /// Inspect bounded history usage.
    /// Takes no arguments and returns group count and serialized delta bytes for
    /// both branches. Groups retain changed entities, not whole project snapshots.
    pub fn history_usage(&self) -> (usize, usize) {
        (
            self.undo.len() + self.redo.len(),
            self.undo
                .iter()
                .chain(&self.redo)
                .map(|group| group.bytes)
                .sum(),
        )
    }

    /// Apply a complete reversible command group.
    /// `expected` owns identity/revision; `label` names the undo step; `commands`
    /// contains 1..64 commands in dependency order. Returns one revision and changed
    /// dependencies. Every intermediate state must validate; rejection changes nothing.
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
        let mut next = (*self.project).clone();
        for command in commands {
            command.apply(&mut next)?;
        }
        let changed = next != *self.project;
        let impact = if changed {
            next.revision = self.next_revision()?;
            let group = GroupChange::between(&self.project, &next, label)?;
            let impact = group.impact.clone();
            self.redo.clear();
            self.undo.push(Arc::new(group));
            while self.undo.len() > HISTORY_LIMIT || self.history_usage().1 > HISTORY_BYTES {
                self.undo.remove(0);
            }
            self.project = Arc::new(next);
            impact
        } else {
            ChangeImpact::default()
        };
        Ok(self.receipt(expected, changed, impact))
    }

    /// Undo the last complete group with a new revision.
    /// `expected` must own this editor. Returns changed dependencies; older job
    /// ownership never becomes current again because revisions keep increasing.
    pub fn undo(&mut self, expected: DocumentVersion) -> Result<CommandReceipt> {
        self.check(expected)?;
        let group = self
            .undo
            .last()
            .ok_or_else(|| Error::Invalid("no command to undo".into()))?;
        let mut next = (*self.project).clone();
        group.restore(&mut next, false);
        next.revision = self.next_revision()?;
        next.validate()?;
        let impact = group.impact.clone();
        let group = self.undo.pop().unwrap();
        self.redo.push(group);
        self.project = Arc::new(next);
        Ok(self.receipt(expected, true, impact))
    }

    /// Redo the last undone group with a new revision.
    /// `expected` must own this editor. Returns updated ownership after validation.
    pub fn redo(&mut self, expected: DocumentVersion) -> Result<CommandReceipt> {
        self.check(expected)?;
        let group = self
            .redo
            .last()
            .ok_or_else(|| Error::Invalid("no command to redo".into()))?;
        let mut next = (*self.project).clone();
        group.restore(&mut next, true);
        next.revision = self.next_revision()?;
        next.validate()?;
        let impact = group.impact.clone();
        let group = self.redo.pop().unwrap();
        self.undo.push(group);
        self.project = Arc::new(next);
        Ok(self.receipt(expected, true, impact))
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
    fn receipt(
        &self,
        before: DocumentVersion,
        changed: bool,
        impact: ChangeImpact,
    ) -> CommandReceipt {
        CommandReceipt {
            before,
            after: DocumentVersion::of(&self.project),
            changed,
            impact,
        }
    }
}
