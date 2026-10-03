//! Optional native reference execution; no Python runtime or research subprocess.

#[cfg(feature = "libtorch")]
#[allow(unsafe_code)]
mod native {
    use sha2::{Digest, Sha256};
    use std::{
        ffi::{CStr, c_char, c_void},
        path::Path,
        ptr::NonNull,
    };

    pub const TORCHSCRIPT_SHA256: &str =
        "f01e0c9338b9a6a31b881ea6d4360d70c1e549701b3792e14c9ed88d6196c5a1";

    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error("reference artifact/input: {0}")]
        Invalid(String),
        #[error("libtorch reference: {0}")]
        Native(String),
        #[error("reference filesystem: {0}")]
        Io(#[from] std::io::Error),
    }

    unsafe extern "C" {
        fn eb_reference_open(
            bytes: *const u8,
            size: usize,
            error: *mut c_char,
            length: usize,
        ) -> *mut c_void;
        fn eb_reference_run(
            handle: *mut c_void,
            rgb: *const f32,
            width: i32,
            height: i32,
            ratio: f32,
            alpha: *mut f32,
            foreground: *mut f32,
            error: *mut c_char,
            length: usize,
        ) -> i32;
        fn eb_reference_close(handle: *mut c_void);
    }

    fn message(error: &[u8]) -> Error {
        Error::Native(
            CStr::from_bytes_until_nul(error)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "invalid native error".into()),
        )
    }

    pub struct ReferenceRvm {
        handle: NonNull<c_void>,
        width: usize,
        height: usize,
        ratio: f32,
    }

    impl ReferenceRvm {
        /// Load the official independent TorchScript reference.
        /// `path` must match its pinned digest. Dimensions and ratio belong to one ordered
        /// shot. Returns exclusive native recurrent state; verified bytes are loaded directly.
        pub fn open(path: &Path, width: usize, height: usize, ratio: f32) -> Result<Self, Error> {
            if width == 0
                || height == 0
                || width > 3840
                || height > 2160
                || !ratio.is_finite()
                || !(0.05..=1.0).contains(&ratio)
            {
                return Err(Error::Invalid("unsupported shot profile".into()));
            }
            let file = std::fs::File::open(path)?;
            if file.metadata()?.len() > 32 * 1024 * 1024 {
                return Err(Error::Invalid("oversized reference".into()));
            }
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 32 * 1024 * 1024
                || format!("{:x}", Sha256::digest(&bytes)) != TORCHSCRIPT_SHA256
            {
                return Err(Error::Invalid("reference digest differs".into()));
            }
            let mut error = [0u8; 2048];
            let pointer = unsafe {
                eb_reference_open(
                    bytes.as_ptr(),
                    bytes.len(),
                    error.as_mut_ptr().cast(),
                    error.len(),
                )
            };
            Ok(Self {
                handle: NonNull::new(pointer).ok_or_else(|| message(&error))?,
                width,
                height,
                ratio,
            })
        }

        /// Evaluate the next original picture using official TorchScript operators.
        /// `rgba` contains one unchanged source picture. Returns alpha and foreground CHW.
        pub fn process(&mut self, rgba: &[u8]) -> Result<(Vec<f32>, Vec<f32>), Error> {
            let pixels = self.width * self.height;
            if rgba.len() != pixels * 4 {
                return Err(Error::Invalid("picture shape differs".into()));
            }
            let mut rgb = vec![0.0; pixels * 3];
            for (i, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
                for c in 0..3 {
                    rgb[c * pixels + i] = f32::from(pixel[c]) / 255.0;
                }
            }
            let mut alpha = vec![0.0; pixels];
            let mut foreground = vec![0.0; pixels * 3];
            let mut error = [0u8; 2048];
            let result = unsafe {
                eb_reference_run(
                    self.handle.as_ptr(),
                    rgb.as_ptr(),
                    self.width as i32,
                    self.height as i32,
                    self.ratio,
                    alpha.as_mut_ptr(),
                    foreground.as_mut_ptr(),
                    error.as_mut_ptr().cast(),
                    error.len(),
                )
            };
            if result < 0 {
                return Err(message(&error));
            }
            if alpha.iter().chain(&foreground).any(|v| !v.is_finite()) {
                return Err(Error::Invalid("nonfinite native output".into()));
            }
            Ok((alpha, foreground))
        }
    }

    impl Drop for ReferenceRvm {
        fn drop(&mut self) {
            unsafe {
                eb_reference_close(self.handle.as_ptr());
            }
        }
    }
}

#[cfg(feature = "libtorch")]
pub use native::{Error, ReferenceRvm, TORCHSCRIPT_SHA256};
