use crate::{
    Cancellation, Error, PictureBudget, Result,
    pictures::{Key, selection},
};
use editbay_core::{
    DocumentVersion, EvaluationSnapshot, FrameRange, SourcePosition, SourceRequest,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

const MAXIMUM_WORK: usize = 1_000_000;

mod pack;
pub(crate) use pack::{Manifest, StoreReader};
pub use pack::{PreparedStore, StoreProgress, StoreSpace, StoreUsage};

/// Disk and entry ceilings for exact source-picture preparation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreBudget {
    pub bytes: u64,
    pub entries: usize,
}

impl Default for StoreBudget {
    fn default() -> Self {
        Self {
            bytes: 8 * 1024 * 1024 * 1024,
            entries: 4096,
        }
    }
}

impl StoreBudget {
    /// Validate fixed storage limits before planning or opening any cache file.
    /// Takes these limits. Returns an error above 8 GiB/4096 entries or at zero.
    pub fn validate(self) -> Result<()> {
        if self.bytes == 0
            || self.bytes > 8 * 1024 * 1024 * 1024
            || !(1..=4096).contains(&self.entries)
        {
            return Err(Error::Invalid(
                "picture store exceeds supported storage limits".into(),
            ));
        }
        Ok(())
    }
}

/// A captured range's exact unique source requests in useful native decode order.
pub struct StorePlan {
    snapshot: Arc<EvaluationSnapshot>,
    composition: Uuid,
    range: FrameRange,
    budget: StoreBudget,
    pictures: PictureBudget,
    requests: BTreeMap<Key, SourceRequest>,
    sources: HashSet<Uuid>,
    bytes: u64,
    work: usize,
}

/// Actual range ownership and resource requirements, without filesystem effects.
#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    pub version: DocumentVersion,
    pub composition: Uuid,
    pub range: FrameRange,
    pub pictures: usize,
    pub bytes: u64,
    pub evaluated_nodes: usize,
    pub budget: StoreBudget,
}

impl StorePlan {
    /// Inspect deduplicated exact requests in source decode order.
    /// Takes this bounded plan. Returns borrowed requests without changing timeline time.
    pub fn requests(&self) -> impl ExactSizeIterator<Item = &SourceRequest> {
        self.requests.values()
    }
    /// Resolve every picture dependency without changing timeline order or pixels.
    /// `snapshot`, `composition` and `range` own the work; `pictures` and `budget`
    /// retain memory/geometry/storage ceilings; `cancel` interrupts planning.
    /// Returns a bounded deduplicated plan, or fails before promising preparation.
    pub fn new(
        snapshot: Arc<EvaluationSnapshot>,
        composition: Uuid,
        range: FrameRange,
        pictures: PictureBudget,
        budget: StoreBudget,
        cancel: &Cancellation,
    ) -> Result<Self> {
        pictures.validate()?;
        budget.validate()?;
        let scene = snapshot
            .project()
            .compositions
            .iter()
            .find(|c| c.id == composition)
            .ok_or_else(|| Error::Invalid("picture preparation composition is absent".into()))?;
        if range.start >= range.end
            || range.end > scene.duration
            || range.end - range.start > MAXIMUM_WORK as u64
        {
            return Err(Error::Invalid("picture preparation range is empty, outside its composition or exceeds planning limits".into()));
        }
        let mut plan = Self {
            snapshot,
            composition,
            range,
            budget,
            pictures,
            requests: BTreeMap::new(),
            sources: HashSet::new(),
            bytes: 0,
            work: 0,
        };
        for frame in range.start..range.end {
            plan.collect(
                composition,
                SourcePosition::new(
                    i64::try_from(frame).map_err(|e| Error::Invalid(e.to_string()))?,
                    1,
                )?,
                false,
                0,
                pictures,
                cancel,
            )?;
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(plan)
    }

    fn collect(
        &mut self,
        composition: Uuid,
        position: SourcePosition,
        before: bool,
        depth: usize,
        pictures: PictureBudget,
        cancel: &Cancellation,
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if depth >= 32 {
            return Err(Error::Invalid(
                "picture preparation exceeds nesting limits".into(),
            ));
        }
        let snapshot = self.snapshot.clone();
        let scene = snapshot
            .project()
            .compositions
            .iter()
            .find(|c| c.id == composition)
            .ok_or_else(|| Error::Invalid("nested picture composition is absent".into()))?;
        self.work = self
            .work
            .checked_add(scene.nodes.len().max(1))
            .filter(|work| *work <= MAXIMUM_WORK)
            .ok_or_else(|| {
                Error::Invalid("picture preparation exceeds its planning work limit".into())
            })?;
        let frame = snapshot.prepare(composition, position, before)?;
        let nodes: HashMap<_, _> = frame.nodes.iter().map(|node| (node.id, node)).collect();
        let mut pending: Vec<_> = frame.picture.into_iter().collect();
        let mut visited = HashSet::new();
        while let Some(id) = pending.pop() {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !visited.insert(id) {
                continue;
            }
            let node = nodes
                .get(&id)
                .ok_or_else(|| Error::Invalid("picture preparation node is absent".into()))?;
            if !node.active {
                continue;
            }
            pending.extend(node.operation.inputs().into_iter().map(|(id, _)| id));
            match &node.source {
                Some(SourceRequest::Composition {
                    composition,
                    position,
                    reverse,
                }) => self.collect(
                    *composition,
                    *position,
                    *reverse,
                    depth + 1,
                    pictures,
                    cancel,
                )?,
                Some(request @ SourceRequest::Media { .. }) => {
                    if let Some(selected) = selection(&snapshot, request, pictures)? {
                        self.sources.insert(selected.reference.id);
                        if self.sources.len() > pictures.source_handles {
                            return Err(Error::Invalid(
                                "picture preparation exceeds source handle limits".into(),
                            ));
                        }
                        if self.requests.contains_key(&selected.key) {
                            continue;
                        }
                        let bytes = self
                            .bytes
                            .checked_add(selected.size as u64)
                            .filter(|bytes| *bytes <= self.budget.bytes)
                            .ok_or_else(|| {
                                Error::Invalid(
                                    "picture preparation exceeds its disk byte limit".into(),
                                )
                            })?;
                        if self.requests.len() == self.budget.entries {
                            return Err(Error::Invalid(
                                "picture preparation exceeds its entry limit".into(),
                            ));
                        }
                        self.requests.insert(selected.key, request.clone());
                        self.bytes = bytes;
                    }
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Inspect the complete bounded plan before a preparation worker is started.
    /// Takes this plan. Returns captured ownership, exact counts and declared limits.
    pub fn summary(&self) -> PlanSummary {
        PlanSummary {
            version: DocumentVersion::of(self.snapshot.project()),
            composition: self.composition,
            range: self.range,
            pictures: self.requests.len(),
            bytes: self.bytes,
            evaluated_nodes: self.work,
            budget: self.budget,
        }
    }
}
