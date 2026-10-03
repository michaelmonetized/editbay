use crate::{Error, Result};
use ort::{
    session::{RunOptions, Session, SessionInputValue},
    value::Tensor,
};

static INIT: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct CancelState {
    requested: AtomicBool,
    native_termination: AtomicBool,
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<CancelState>);

impl Cancellation {
    /// Request cancellation of this job.
    /// Takes no arguments; returns after setting the flag observed by native operators.
    pub fn cancel(&self) {
        self.0.requested.store(true, Ordering::Release);
    }
    /// Read cancellation state.
    /// Takes no arguments and returns whether a caller requested cancellation.
    pub fn cancelled(&self) -> bool {
        self.0.requested.load(Ordering::Acquire)
    }
    /// Inspect native cancellation delivery.
    /// Takes no arguments and returns whether ONNX Runtime accepted a termination flag.
    pub fn native_termination_requested(&self) -> bool {
        self.0.native_termination.load(Ordering::Acquire)
    }
    /// Reject work after cancellation.
    /// Takes no arguments; returns `Cancelled` once the job has been cancelled.
    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FloatTensor {
    pub shape: Vec<usize>,
    pub values: Vec<f32>,
}

impl FloatTensor {
    /// Validate an owned inference tensor.
    /// Returns success only for finite floats, matching shape and bounded allocation.
    pub fn validate(&self) -> Result<()> {
        let elements = self
            .shape
            .iter()
            .try_fold(1usize, |n, &size| n.checked_mul(size));
        if self.shape.len() > 6
            || elements != Some(self.values.len())
            || self.values.is_empty()
            || self.values.len() > 16_777_216
            || self.values.iter().any(|v| !v.is_finite())
        {
            return Err(Error::Invalid("invalid or oversized float tensor".into()));
        }
        Ok(())
    }
}

/// Initialize an explicit native runtime.
/// `library` points to the selected ONNX Runtime shared library. Returns a process-wide
/// binding; a later attempt to switch libraries fails rather than silently reusing it.
pub fn initialize(library: &Path) -> Result<()> {
    let path = library.canonicalize()?;
    let selected = INIT
        .get_or_init(|| {
            ort::init_from(&path)
                .map_err(|e| e.to_string())?
                .with_name("EditBay native feasibility")
                .commit();
            Ok(path.clone())
        })
        .as_ref()
        .map_err(|e| Error::Runtime(e.clone()))?;
    if selected != &path {
        return Err(Error::Invalid(
            "native runtime already selected for this process".into(),
        ));
    }
    Ok(())
}

/// Inspect the selected native runtime build.
/// Takes no arguments and returns ONNX Runtime's own version, commit and build flags.
/// Fails before explicit runtime initialization rather than triggering a default loader.
pub fn runtime_build_info() -> Result<&'static str> {
    if !matches!(INIT.get(), Some(Ok(_))) {
        return Err(Error::Invalid(
            "select an explicit native runtime first".into(),
        ));
    }
    Ok(ort::info())
}

pub struct ModelSession {
    session: Option<Session>,
    #[cfg(feature = "tract-reference")]
    reference: Option<crate::reference::TractGraph>,
    pub sha256: String,
}

impl ModelSession {
    /// Load a verified native artifact.
    /// `path` is local; `sha256` is the approved manifest digest. Returns a CPU session
    /// using two inference threads. The model bytes verified are exactly those loaded.
    pub fn load(path: &Path, sha256: &str, cancellation: &Cancellation) -> Result<Self> {
        cancellation.check()?;
        if !matches!(INIT.get(), Some(Ok(_))) {
            return Err(Error::Invalid(
                "select an explicit native runtime before loading models".into(),
            ));
        }
        let bytes = model_bytes(path, sha256, cancellation)?;
        let session = Session::builder()
            .map_err(runtime_error)?
            .with_intra_threads(2)
            .map_err(runtime_error)?
            .commit_from_memory(&bytes)
            .map_err(runtime_error)?;
        cancellation.check()?;
        Ok(Self {
            session: Some(session),
            #[cfg(feature = "tract-reference")]
            reference: None,
            sha256: sha256.into(),
        })
    }

    /// Load an independent Rust reference executor.
    /// `path` and `sha256` select exact verified graph bytes. Returns Tract kernels for
    /// numerical comparison. Reference runs check cancellation at graph boundaries only.
    #[cfg(feature = "tract-reference")]
    pub fn load_reference(path: &Path, sha256: &str, cancellation: &Cancellation) -> Result<Self> {
        let bytes = model_bytes(path, sha256, cancellation)?;
        let reference = crate::reference::TractGraph::open(bytes)?;
        cancellation.check()?;
        Ok(Self {
            session: None,
            reference: Some(reference),
            sha256: sha256.into(),
        })
    }

    /// Run named float inputs with actual operator cancellation.
    /// `inputs` are owned tensors; `cancellation` reaches ONNX Runtime's termination flag.
    /// Returns named finite owned outputs after the complete native run succeeds.
    pub fn run(
        &mut self,
        inputs: Vec<(&str, FloatTensor)>,
        cancellation: &Cancellation,
    ) -> Result<BTreeMap<String, FloatTensor>> {
        self.run_with_labels(inputs, None, cancellation)
    }

    /// Run named floats and optional integer prompt labels.
    /// `labels` supplies a matching shape and INT32 values. Cancellation reaches native
    /// operators. Returns finite owned float outputs only after a complete successful run.
    pub fn run_with_labels(
        &mut self,
        inputs: Vec<(&str, FloatTensor)>,
        labels: Option<(&str, Vec<usize>, Vec<i32>)>,
        cancellation: &Cancellation,
    ) -> Result<BTreeMap<String, FloatTensor>> {
        cancellation.check()?;
        #[cfg(feature = "tract-reference")]
        if let Some(reference) = &mut self.reference {
            return reference.run(inputs, labels, cancellation);
        }
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| Error::Runtime("native session absent".into()))?;
        let mut tensors: Vec<(&str, SessionInputValue<'_>)> = Vec::with_capacity(inputs.len() + 1);
        for (name, input) in inputs {
            input.validate()?;
            tensors.push((
                name,
                Tensor::from_array((input.shape, input.values))
                    .map_err(runtime_error)?
                    .into(),
            ));
        }
        if let Some((name, shape, values)) = labels {
            if shape.iter().try_fold(1usize, |n, &s| n.checked_mul(s)) != Some(values.len())
                || values.is_empty()
                || values.len() > 64
            {
                return Err(Error::Invalid("invalid integer prompt labels".into()));
            }
            tensors.push((
                name,
                Tensor::from_array((shape, values))
                    .map_err(runtime_error)?
                    .into(),
            ));
        }
        let options = RunOptions::new().map_err(runtime_error)?;
        let finished = AtomicBool::new(false);
        struct Finish<'a>(&'a AtomicBool);
        impl Drop for Finish<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let result = std::thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(Ordering::Acquire) {
                    if cancellation.cancelled() {
                        if options.terminate().is_ok() {
                            cancellation
                                .0
                                .native_termination
                                .store(true, Ordering::Release);
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            let _finish = Finish(&finished);
            let outputs = session
                .run_with_options::<_, 0>(tensors, &options)
                .map_err(runtime_error)?;
            let mut owned = BTreeMap::new();
            for (name, output) in outputs.iter() {
                let (shape, values) = output.try_extract_tensor::<f32>().map_err(runtime_error)?;
                let tensor = FloatTensor {
                    shape: shape
                        .iter()
                        .map(|&n| {
                            usize::try_from(n)
                                .map_err(|_| Error::Invalid("negative output dimension".into()))
                        })
                        .collect::<Result<_>>()?,
                    values: values.to_vec(),
                };
                tensor.validate()?;
                owned.insert(name.into(), tensor);
            }
            Ok(owned)
        });
        cancellation.check()?;
        result
    }
}

fn model_bytes(path: &Path, sha256: &str, cancellation: &Cancellation) -> Result<Vec<u8>> {
    cancellation.check()?;
    if File::open(path)?.metadata()?.len() > 512 * 1024 * 1024 {
        return Err(Error::Invalid(
            "model exceeds the feasibility pack budget".into(),
        ));
    }
    let mut bytes = Vec::new();
    let mut input = File::open(path)?.take(512 * 1024 * 1024 + 1);
    let mut block = [0u8; 65536];
    loop {
        cancellation.check()?;
        let n = input.read(&mut block)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&block[..n]);
        if bytes.len() > 512 * 1024 * 1024 {
            return Err(Error::Invalid(
                "model exceeds the feasibility pack budget".into(),
            ));
        }
    }
    if format!("{:x}", Sha256::digest(&bytes)) != sha256 {
        return Err(Error::Integrity);
    }
    Ok(bytes)
}

fn runtime_error(error: impl std::fmt::Display) -> Error {
    Error::Runtime(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape_overflow_nonfinite_data_and_cancelled_jobs_fail_before_native_work() {
        assert!(
            FloatTensor {
                shape: vec![usize::MAX, 2],
                values: vec![0.0]
            }
            .validate()
            .is_err()
        );
        assert!(
            FloatTensor {
                shape: vec![1],
                values: vec![f32::NAN]
            }
            .validate()
            .is_err()
        );
        assert!(
            FloatTensor {
                shape: vec![2],
                values: vec![0.0]
            }
            .validate()
            .is_err()
        );
        let cancel = Cancellation::default();
        cancel.cancel();
        assert!(matches!(cancel.check(), Err(Error::Cancelled)));
        assert!(matches!(
            ModelSession::load(Path::new("/nonexistent"), "bad", &cancel),
            Err(Error::Cancelled)
        ));
    }
}
