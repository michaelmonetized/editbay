//! Each handle exclusively owns its C resources. Buffers carry their exact length;
//! sound uses aligned float storage. C retains neither paths nor Rust buffers.
use crate::{Cancellation, Error, MediaInfo, Result};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs::{File, OpenOptions},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::Path,
    ptr::NonNull,
    sync::Arc,
};

unsafe extern "C" {
    fn eb_reader_open(
        descriptor: c_int,
        path: *const c_char,
        stream: c_int,
        mode: c_int,
        cancel: *mut c_void,
        info: *mut MediaInfo,
        details: *mut RawStream,
        error: *mut c_int,
    ) -> *mut c_void;
    fn eb_reader_close(reader: *mut c_void);
    fn eb_reader_next(
        reader: *mut c_void,
        output: *mut u8,
        capacity: usize,
        details: *mut RawFrame,
    ) -> c_int;
    fn eb_reader_seek(reader: *mut c_void, tick: i64) -> c_int;
    fn eb_reader_picture_at(
        reader: *mut c_void,
        tick: i64,
        output: *mut u8,
        capacity: usize,
        details: *mut RawFrame,
    ) -> c_int;
    fn eb_cancel_create() -> *mut c_void;
    fn eb_cancel_destroy(cancel: *mut c_void);
    fn eb_cancel_request(cancel: *mut c_void);
    fn eb_cancelled(cancel: *mut c_void) -> c_int;
    fn eb_probe_open(
        descriptor: c_int,
        path: *const c_char,
        cancel: *mut c_void,
        error: *mut c_int,
    ) -> *mut c_void;
    fn eb_probe_close(probe: *mut c_void);
    fn eb_probe_streams(probe: *mut c_void) -> c_int;
    fn eb_probe_info(
        probe: *mut c_void,
        stream: c_int,
        details: *mut RawStream,
        codec: *mut *const c_char,
    ) -> c_int;
    fn eb_probe_metadata(
        probe: *mut c_void,
        stream: c_int,
        index: c_int,
        key: *mut *const c_char,
        value: *mut *const c_char,
    ) -> c_int;
    fn eb_probe_channel(
        probe: *mut c_void,
        stream: c_int,
        index: c_int,
        name: *mut c_char,
        capacity: usize,
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

fn check(code: i32, cancel: &Cancellation) -> Result<i32> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else if code < 0 {
        Err(error(code))
    } else {
        Ok(code)
    }
}

fn path_string(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| Error::Invalid("path contains NUL".into()))
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct RawStream {
    pub index: i32,
    pub kind: i32,
    pub width: i32,
    pub height: i32,
    pub rate_num: i32,
    pub rate_den: i32,
    pub aspect_num: i32,
    pub aspect_den: i32,
    pub base_num: i32,
    pub base_den: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub channel_order: i32,
    pub pixel_format: i32,
    pub color_primaries: i32,
    pub color_transfer: i32,
    pub color_matrix: i32,
    pub color_range: i32,
    pub alpha_mode: i32,
    pub has_alpha: i32,
    pub disposition: i32,
    pub decoder_available: i32,
    pub start_tick: i64,
    pub duration_ticks: i64,
    pub rotation: f64,
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct RawFrame {
    pub pts: i64,
    pub duration: i64,
    pub sample_start: i64,
    pub color_primaries: i32,
    pub color_transfer: i32,
    pub color_matrix: i32,
    pub color_range: i32,
    pub alpha_mode: i32,
    pub has_alpha: i32,
}

struct CancelInner(NonNull<c_void>);

unsafe impl Send for CancelInner {}
unsafe impl Sync for CancelInner {}

impl Drop for CancelInner {
    fn drop(&mut self) {
        unsafe { eb_cancel_destroy(self.0.as_ptr()) }
    }
}

#[derive(Clone)]
pub(crate) struct Cancel(Arc<CancelInner>);

impl Cancel {
    pub(crate) fn new() -> Result<Self> {
        let pointer = NonNull::new(unsafe { eb_cancel_create() })
            .ok_or_else(|| Error::Invalid("unable to allocate cancellation token".into()))?;
        Ok(Self(Arc::new(CancelInner(pointer))))
    }

    pub(crate) fn request(&self) {
        unsafe { eb_cancel_request(self.0.0.as_ptr()) }
    }

    pub(crate) fn requested(&self) -> bool {
        unsafe { eb_cancelled(self.0.0.as_ptr()) != 0 }
    }

    fn pointer(&self) -> *mut c_void {
        self.0.0.as_ptr()
    }
}

pub(crate) struct Probe {
    pointer: NonNull<c_void>,
    cancel: Cancellation,
}

impl Probe {
    pub(crate) fn open(file: &File, label: &Path, cancel: Cancellation) -> Result<Self> {
        let label = path_string(label)?;
        let mut code = 0;
        let pointer = unsafe {
            eb_probe_open(
                file.as_raw_fd(),
                label.as_ptr(),
                cancel.0.pointer(),
                &mut code,
            )
        };
        let pointer = NonNull::new(pointer)
            .ok_or_else(|| check(code, &cancel).err().unwrap_or_else(|| error(code)))?;
        Ok(Self { pointer, cancel })
    }

    pub(crate) fn streams(&self) -> Result<usize> {
        let count = check(
            unsafe { eb_probe_streams(self.pointer.as_ptr()) },
            &self.cancel,
        )?;
        if count > 256 {
            return Err(Error::Invalid("source exceeds 256 streams".into()));
        }
        Ok(count as usize)
    }

    pub(crate) fn info(&self, stream: usize) -> Result<(RawStream, String)> {
        let mut raw = RawStream::default();
        let mut codec = std::ptr::null();
        check(
            unsafe { eb_probe_info(self.pointer.as_ptr(), stream as i32, &mut raw, &mut codec) },
            &self.cancel,
        )?;
        Ok((raw, bounded_text(codec, 256)?))
    }

    pub(crate) fn metadata(
        &self,
        stream: i32,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let mut result = std::collections::BTreeMap::new();
        for index in 0..=128 {
            let mut key = std::ptr::null();
            let mut value = std::ptr::null();
            let count = check(
                unsafe {
                    eb_probe_metadata(self.pointer.as_ptr(), stream, index, &mut key, &mut value)
                },
                &self.cancel,
            )?;
            if count == 0 {
                return Ok(result);
            }
            if index == 128 {
                return Err(Error::Invalid("source exceeds 128 metadata fields".into()));
            }
            result.insert(bounded_text(key, 1024)?, bounded_text(value, 16_384)?);
        }
        unreachable!()
    }

    pub(crate) fn channels(&self, stream: usize, count: i32) -> Result<Vec<String>> {
        if !(1..=64).contains(&count) {
            return Err(Error::Invalid("source channel count must be 1..64".into()));
        }
        (0..count)
            .map(|index| {
                let mut bytes = [0u8; 64];
                let length = check(
                    unsafe {
                        eb_probe_channel(
                            self.pointer.as_ptr(),
                            stream as i32,
                            index,
                            bytes.as_mut_ptr().cast(),
                            bytes.len(),
                        )
                    },
                    &self.cancel,
                )?;
                if length as usize >= bytes.len() {
                    return Err(Error::Invalid(
                        "source channel name exceeds 63 bytes".into(),
                    ));
                }
                Ok(CStr::from_bytes_until_nul(&bytes)
                    .map_err(|_| Error::Invalid("invalid channel name".into()))?
                    .to_string_lossy()
                    .into_owned())
            })
            .collect()
    }
}

fn bounded_text(pointer: *const c_char, limit: usize) -> Result<String> {
    if pointer.is_null() {
        return Err(Error::Invalid("missing codec metadata".into()));
    }
    let length = unsafe { libc::strnlen(pointer, limit + 1) };
    if length > limit {
        return Err(Error::Invalid(
            "codec metadata exceeds its byte budget".into(),
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) };
    String::from_utf8(bytes.to_vec())
        .map_err(|_| Error::Invalid("source metadata is not UTF-8".into()))
}

impl Drop for Probe {
    fn drop(&mut self) {
        unsafe { eb_probe_close(self.pointer.as_ptr()) }
    }
}

pub(crate) struct Reader {
    pointer: NonNull<c_void>,
    cancel: Cancellation,
    pub(crate) stream: RawStream,
}

impl Reader {
    pub(crate) fn open(path: &Path, audio: bool) -> Result<(Self, MediaInfo)> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        Self::from_file(&file, path, None, i32::from(audio), Cancellation::new()?)
    }

    pub(crate) fn from_file(
        file: &File,
        path: &Path,
        stream: Option<u32>,
        mode: i32,
        cancel: Cancellation,
    ) -> Result<(Self, MediaInfo)> {
        if !file.metadata()?.is_file() {
            return Err(Error::Invalid("source must be a local regular file".into()));
        }
        let path = path_string(path)?;
        let selected = stream
            .map(i32::try_from)
            .transpose()
            .map_err(|_| Error::Invalid("invalid stream index".into()))?
            .unwrap_or(-1);
        let mut info = MediaInfo::default();
        let mut details = RawStream::default();
        let mut code = 0;
        let pointer = unsafe {
            eb_reader_open(
                file.as_raw_fd(),
                path.as_ptr(),
                selected,
                mode,
                cancel.0.pointer(),
                &mut info,
                &mut details,
                &mut code,
            )
        };
        let pointer = NonNull::new(pointer)
            .ok_or_else(|| check(code, &cancel).err().unwrap_or_else(|| error(code)))?;
        let reader = Self {
            pointer,
            cancel,
            stream: details,
        };
        if details.base_num <= 0 || details.base_den <= 0 {
            return Err(Error::Invalid(
                "selected stream has no valid time base".into(),
            ));
        }
        Ok((reader, info))
    }

    pub(crate) fn next(&mut self, output: &mut [u8]) -> Result<(usize, RawFrame)> {
        let mut details = RawFrame::default();
        let count = unsafe {
            eb_reader_next(
                self.pointer.as_ptr(),
                output.as_mut_ptr(),
                output.len(),
                &mut details,
            )
        };
        Ok((check(count, &self.cancel)? as usize, details))
    }

    pub(crate) fn picture_at(&mut self, tick: i64, output: &mut [u8]) -> Result<(usize, RawFrame)> {
        let mut details = RawFrame::default();
        let count = unsafe {
            eb_reader_picture_at(
                self.pointer.as_ptr(),
                tick,
                output.as_mut_ptr(),
                output.len(),
                &mut details,
            )
        };
        Ok((check(count, &self.cancel)? as usize, details))
    }

    pub(crate) fn next_audio(&mut self, output: &mut [f32]) -> Result<(usize, RawFrame)> {
        let mut details = RawFrame::default();
        let count = unsafe {
            eb_reader_next(
                self.pointer.as_ptr(),
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(output),
                &mut details,
            )
        };
        Ok((check(count, &self.cancel)? as usize, details))
    }

    pub(crate) fn next_timing(&mut self) -> Result<Option<RawFrame>> {
        let mut details = RawFrame::default();
        let count = check(
            unsafe { eb_reader_next(self.pointer.as_ptr(), std::ptr::null_mut(), 0, &mut details) },
            &self.cancel,
        )?;
        Ok((count != 0).then_some(details))
    }

    pub(crate) fn seek(&mut self, tick: i64) -> Result<()> {
        check(
            unsafe { eb_reader_seek(self.pointer.as_ptr(), tick) },
            &self.cancel,
        )?;
        Ok(())
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            eb_reader_close(self.pointer.as_ptr());
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

pub(crate) fn worker_limits() -> Result<()> {
    for (resource, budget) in [
        (libc::RLIMIT_AS, crate::worker::VIRTUAL_MEMORY_BYTES),
        (libc::RLIMIT_CPU, crate::worker::CPU_SECONDS),
        (libc::RLIMIT_NOFILE, 512),
    ] {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(resource, &mut limit) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        limit.rlim_cur = limit.rlim_cur.min(budget).min(limit.rlim_max);
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
