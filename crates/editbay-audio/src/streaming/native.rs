use super::native_trace::{NativeContinuity, NativeTrace};
use super::{Callback, Result};
use std::{
    ffi::{c_int, c_void},
    ptr::NonNull,
    sync::Arc,
};

#[repr(C)]
struct Handle {
    _private: [u8; 0],
}
unsafe extern "C" {
    fn eb_pw_open(
        rate: u32,
        channels: u32,
        quantum: u32,
        render: unsafe extern "C" fn(*mut c_void, *mut f32, usize, u64, u64, *const u64),
        user: *mut c_void,
    ) -> *mut Handle;
    fn eb_pw_active(handle: *mut Handle, active: c_int) -> c_int;
    fn eb_pw_close(handle: *mut Handle);
    fn eb_pw_error(handle: *mut Handle) -> c_int;
    fn eb_pw_frames(handle: *mut Handle) -> u32;
}

struct Owner {
    callback: Callback,
    rate: u32,
    channels: usize,
    trace: Arc<NativeTrace>,
    continuity: NativeContinuity,
}

/// Serialized control handle retaining one exclusively real-time callback owner.
pub(super) struct Stream {
    handle: NonNull<Handle>,
    _owner: Box<Owner>,
}

impl Stream {
    /// Connect inactive native float playback with an owned pre-rendered sound ring.
    /// `rate`, `channels`, `quantum` and `callback` select layout and lifetime.
    /// Returns a negotiated stream; no Rust callback runs on the control thread.
    pub(super) fn new(rate: u32, channels: u16, quantum: u32, callback: Callback) -> Result<Self> {
        let mut owner = Box::new(Owner {
            callback,
            rate,
            channels: usize::from(channels),
            trace: Arc::new(NativeTrace::default()),
            continuity: NativeContinuity::default(),
        });
        let handle = unsafe {
            eb_pw_open(
                rate,
                u32::from(channels),
                quantum,
                render,
                (&mut *owner as *mut Owner).cast(),
            )
        };
        let handle = NonNull::new(handle)
            .ok_or_else(|| format!("Native PipeWire setup: {}", std::io::Error::last_os_error()))?;
        Ok(Self {
            handle,
            _owner: owner,
        })
    }
    /// Set native stream activity through PipeWire's serialized control loop.
    /// `active` selects playback or pause; returns the actual native outcome.
    pub(super) fn active(&self, active: bool) -> Result<()> {
        let result = unsafe { eb_pw_active(self.handle.as_ptr(), i32::from(active)) };
        if result < 0 {
            Err(format!(
                "Native PipeWire activity: {}",
                std::io::Error::from_raw_os_error(-result)
            ))
        } else {
            Ok(())
        }
    }
    /// Inspect the latest native callback size without touching callback state.
    /// Takes no arguments and returns the negotiated/requested frame count.
    pub(super) fn frames(&self) -> u32 {
        unsafe { eb_pw_frames(self.handle.as_ptr()) }
    }
    /// Retain bounded native clock and buffer observations for control diagnostics.
    /// Takes no arguments and returns the callback's shared atomic trace.
    pub(super) fn trace(&self) -> Arc<NativeTrace> {
        self._owner.trace.clone()
    }
    /// Inspect a terminal native layout, server or timestamp failure.
    /// Takes no arguments and returns the native error without waiting on its loop.
    pub(super) fn check(&self) -> Result<()> {
        let error = unsafe { eb_pw_error(self.handle.as_ptr()) };
        if error == 0 {
            Ok(())
        } else {
            Err(format!(
                "Native PipeWire playback: {}",
                std::io::Error::from_raw_os_error(error)
            ))
        }
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            eb_pw_close(self.handle.as_ptr());
        }
    }
}

/// Consume prepared sound on PipeWire's exclusive real-time data thread.
/// `user` retains the owner; output geometry and timestamps come from validated
/// native buffers. Returns after a bounded allocation-free float ring transfer.
unsafe extern "C" fn render(
    user: *mut c_void,
    output: *mut f32,
    samples: usize,
    callback: u64,
    playback: u64,
    diagnostic: *const u64,
) {
    let owner = unsafe { &mut *user.cast::<Owner>() };
    let diagnostic = unsafe { &*diagnostic.cast::<[u64; 17]>() };
    owner.trace.record(diagnostic);
    if !owner
        .continuity
        .observe(diagnostic[14], diagnostic[6], diagnostic[4])
    {
        owner.callback.control.stop(super::State::BackendUnderrun);
        owner.callback.cancel.cancel();
    }
    let output = unsafe { std::slice::from_raw_parts_mut(output, samples) };
    let instant =
        |ns: u64| cpal::StreamInstant::new(ns / 1_000_000_000, (ns % 1_000_000_000) as u32);
    owner.callback.fill(
        output,
        owner.channels,
        owner.rate,
        cpal::OutputStreamTimestamp {
            callback: instant(callback),
            playback: instant(playback),
        },
    );
}
