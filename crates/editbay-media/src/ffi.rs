//! Each handle exclusively owns its C resources. Buffers carry their exact length;
//! sound uses aligned float storage. C retains neither paths nor Rust buffers.
use crate::{Error, MediaInfo, Result};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs::OpenOptions,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::Path,
    ptr::NonNull,
};

unsafe extern "C" {
    fn eb_reader_open(
        path: *const c_char,
        audio: c_int,
        info: *mut MediaInfo,
        error: *mut c_int,
    ) -> *mut c_void;
    fn eb_reader_close(reader: *mut c_void);
    fn eb_reader_next(
        reader: *mut c_void,
        output: *mut u8,
        capacity: usize,
        pts: *mut i64,
    ) -> c_int;
    fn eb_writer_open(
        descriptor: c_int,
        width: c_int,
        height: c_int,
        num: c_int,
        den: c_int,
        error: *mut c_int,
    ) -> *mut c_void;
    fn eb_writer_close(writer: *mut c_void);
    fn eb_writer_frame(writer: *mut c_void, rgba: *const u8, length: usize) -> c_int;
    fn eb_writer_finish(writer: *mut c_void) -> c_int;
    fn eb_error(code: c_int, message: *mut c_char, length: usize);
    fn eb_version() -> *const c_char;
}

fn error(code: i32) -> Error {
    let mut bytes = [0u8; 256];
    unsafe {
        eb_error(code, bytes.as_mut_ptr().cast(), bytes.len());
    }
    let message = CStr::from_bytes_until_nul(&bytes)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("error {code}"));
    Error::Codec(message)
}

fn path_string(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| Error::Invalid("path contains NUL".into()))
}

pub(crate) struct Reader(NonNull<c_void>);

impl Reader {
    pub(crate) fn open(path: &Path, audio: bool) -> Result<(Self, MediaInfo)> {
        if !path.is_file() {
            return Err(Error::Invalid("source must be a local regular file".into()));
        }
        let path = path_string(path)?;
        let mut info = MediaInfo::default();
        let mut code = 0;
        let pointer =
            unsafe { eb_reader_open(path.as_ptr(), i32::from(audio), &mut info, &mut code) };
        let inner = NonNull::new(pointer).ok_or_else(|| error(code))?;
        Ok((Self(inner), info))
    }

    pub(crate) fn next(&mut self, output: &mut [u8]) -> Result<(usize, Option<i64>)> {
        let mut timestamp = i64::MIN;
        let count = unsafe {
            eb_reader_next(
                self.0.as_ptr(),
                output.as_mut_ptr(),
                output.len(),
                &mut timestamp,
            )
        };
        if count < 0 {
            return Err(error(count));
        }
        Ok((count as usize, (timestamp != i64::MIN).then_some(timestamp)))
    }

    pub(crate) fn next_audio(&mut self, output: &mut [f32]) -> Result<usize> {
        let mut timestamp = i64::MIN;
        let count = unsafe {
            eb_reader_next(
                self.0.as_ptr(),
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(output),
                &mut timestamp,
            )
        };
        if count < 0 {
            Err(error(count))
        } else {
            Ok(count as usize)
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            eb_reader_close(self.0.as_ptr());
        }
    }
}

pub struct VideoWriter(NonNull<c_void>);

impl VideoWriter {
    /// Encode a prototype lossless picture stream.
    /// `path` must be a caller-owned temporary file; `info` supplies dimensions and rational rate.
    /// Returns an FFV1/Matroska encoder. Final publication belongs to the caller.
    pub fn open_temporary(path: &Path, info: MediaInfo) -> Result<Self> {
        if info.width <= 0
            || info.height <= 0
            || info.width > 8192
            || info.height > 8192
            || info.rate_num <= 0
            || info.rate_den <= 0
        {
            return Err(Error::Invalid("invalid export profile".into()));
        }
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?,
            Err(e) => return Err(e.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err(Error::Invalid(
                "encoder requires an empty regular temporary file".into(),
            ));
        }
        let mut code = 0;
        let pointer = unsafe {
            eb_writer_open(
                file.as_raw_fd(),
                info.width,
                info.height,
                info.rate_num,
                info.rate_den,
                &mut code,
            )
        };
        NonNull::new(pointer).map(Self).ok_or_else(|| error(code))
    }

    /// Append an encoded picture.
    /// `rgba` contains one full-range RGBA8 picture. Returns a codec error on failure.
    pub fn write(&mut self, rgba: &[u8]) -> Result<()> {
        let result = unsafe { eb_writer_frame(self.0.as_ptr(), rgba.as_ptr(), rgba.len()) };
        if result < 0 {
            Err(error(result))
        } else {
            Ok(())
        }
    }

    /// Drain and close the stream trailer.
    /// Consumes the encoder and returns only after all frames are muxed.
    pub fn finish(self) -> Result<()> {
        let result = unsafe { eb_writer_finish(self.0.as_ptr()) };
        if result < 0 {
            Err(error(result))
        } else {
            Ok(())
        }
    }
}

impl Drop for VideoWriter {
    fn drop(&mut self) {
        unsafe {
            eb_writer_close(self.0.as_ptr());
        }
    }
}

pub(crate) fn version() -> String {
    unsafe { CStr::from_ptr(eb_version()).to_string_lossy().into_owned() }
}
