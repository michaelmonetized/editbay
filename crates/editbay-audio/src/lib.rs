//! Predecoded, bounded playback with a device-callback-owned sample clock.

use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use serde::Serialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, Serialize)]
pub struct DeviceProfile {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
}

#[derive(Default)]
struct Clock {
    generation: AtomicU64,
    start_ns: AtomicU64,
    submitted_ns: AtomicU64,
    callback_ns: AtomicU64,
    latency_ns: AtomicU64,
    callbacks: AtomicU64,
    failed: AtomicBool,
}

impl Clock {
    fn publish(&self, start_ns: u64, submitted_ns: u64, callback_ns: u64, latency_ns: u64) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.start_ns.store(start_ns, Ordering::SeqCst);
        self.submitted_ns.store(submitted_ns, Ordering::SeqCst);
        self.callback_ns.store(callback_ns, Ordering::SeqCst);
        self.latency_ns.store(latency_ns, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
    }

    fn nanoseconds(&self, now_ns: u64) -> u64 {
        for _ in 0..3 {
            let generation = self.generation.load(Ordering::SeqCst);
            if !generation.is_multiple_of(2) {
                continue;
            }
            let start = self.start_ns.load(Ordering::SeqCst);
            let submitted = self.submitted_ns.load(Ordering::SeqCst);
            let callback = self.callback_ns.load(Ordering::SeqCst);
            let latency = self.latency_ns.load(Ordering::SeqCst);
            if self.generation.load(Ordering::SeqCst) == generation {
                return start
                    .saturating_add(now_ns.saturating_sub(callback))
                    .saturating_sub(latency)
                    .min(submitted);
            }
        }
        self.start_ns
            .load(Ordering::SeqCst)
            .saturating_sub(self.latency_ns.load(Ordering::SeqCst))
    }
}

pub struct Playback {
    stream: cpal::Stream,
    clock: Arc<Clock>,
    origin: Instant,
    pub profile: DeviceProfile,
    source_frames: u64,
}

impl Playback {
    /// Play decoded sound on the actual default output device.
    /// `stereo_48k` is interleaved stereo float sound, capped at 30 seconds.
    /// Returns a live callback clock; the callback allocates nothing and takes no locks.
    pub fn start(stereo_48k: Vec<f32>) -> Result<Self> {
        if stereo_48k.is_empty()
            || !stereo_48k.len().is_multiple_of(2)
            || stereo_48k.len() > 48_000 * 2 * 30
            || stereo_48k.iter().any(|v| !v.is_finite())
        {
            return Err(
                "playback needs finite stereo 48 kHz samples within the 30 s probe budget".into(),
            );
        }
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no output device")?;
        let config = device.default_output_config()?;
        let profile = DeviceProfile {
            name: device.name()?,
            sample_rate: config.sample_rate().0,
            channels: config.channels(),
            sample_format: format!("{:?}", config.sample_format()),
        };
        let clock = Arc::new(Clock::default());
        let origin = Instant::now();
        let source_frames = stereo_48k.len() as u64 / 2;
        let source: Arc<[f32]> = stereo_48k.into();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                build::<f32>(&device, &config.into(), source, clock.clone(), origin)
            }
            cpal::SampleFormat::F64 => {
                build::<f64>(&device, &config.into(), source, clock.clone(), origin)
            }
            cpal::SampleFormat::I16 => {
                build::<i16>(&device, &config.into(), source, clock.clone(), origin)
            }
            cpal::SampleFormat::I32 => {
                build::<i32>(&device, &config.into(), source, clock.clone(), origin)
            }
            cpal::SampleFormat::U16 => {
                build::<u16>(&device, &config.into(), source, clock.clone(), origin)
            }
            other => Err(format!("unsupported output sample format {other:?}").into()),
        }?;
        stream.play()?;
        Ok(Self {
            stream,
            clock,
            origin,
            profile,
            source_frames,
        })
    }

    /// Read the estimated device playback clock.
    /// Returns callback sample time interpolated between buffers, corrected by backend latency.
    /// It cannot advance past submitted sound; physical speaker latency is not measured.
    pub fn nanoseconds(&self) -> u128 {
        u128::from(
            self.clock
                .nanoseconds(self.origin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64),
        )
    }

    pub fn callbacks(&self) -> u64 {
        self.clock.callbacks.load(Ordering::Relaxed)
    }
    pub fn failed(&self) -> bool {
        self.clock.failed.load(Ordering::Relaxed)
    }
    pub fn reported_latency_ns(&self) -> u64 {
        self.clock.latency_ns.load(Ordering::SeqCst)
    }
    pub fn finished(&self) -> bool {
        self.nanoseconds() >= u128::from(self.source_frames) * 1_000_000_000 / 48_000
    }
    pub fn stop(&self) -> Result<()> {
        self.stream.pause()?;
        Ok(())
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    source: Arc<[f32]>,
    clock: Arc<Clock>,
    origin: Instant,
) -> Result<cpal::Stream> {
    let channels = usize::from(config.channels);
    let rate = u64::from(config.sample_rate.0);
    let mut cursor = 0u64;
    let error_clock = clock.clone();
    Ok(device.build_output_stream(
        config,
        move |output: &mut [T], info| {
            let start = cursor;
            let callback_ns = origin.elapsed().as_nanos() as u64;
            let timestamp = info.timestamp();
            let latency_ns = timestamp
                .playback
                .duration_since(&timestamp.callback)
                .map_or(0, |delay| delay.as_nanos() as u64);
            for frame in output.chunks_mut(channels) {
                let position = cursor * 48_000;
                let index = (position / rate) as usize;
                let fraction = (position % rate) as f32 / rate as f32;
                let sample = |channel| {
                    let a = source.get(index * 2 + channel).copied().unwrap_or(0.0);
                    let b = source.get((index + 1) * 2 + channel).copied().unwrap_or(a);
                    (a + (b - a) * fraction).clamp(-1.0, 1.0)
                };
                let left = sample(0);
                let right = sample(1);
                for (channel, value) in frame.iter_mut().enumerate() {
                    *value = T::from_sample(if channels == 1 {
                        (left + right) * 0.5
                    } else if channel == 0 {
                        left
                    } else if channel == 1 {
                        right
                    } else {
                        0.0
                    });
                }
                cursor += 1;
            }
            clock.publish(
                start * 1_000_000_000 / rate,
                cursor * 1_000_000_000 / rate,
                callback_ns,
                latency_ns,
            );
        },
        move |_| {
            error_clock.failed.store(true, Ordering::Relaxed);
        },
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use super::Clock;

    #[test]
    fn clock_interpolates_buffers_with_latency_and_stops_at_submitted_sound() {
        let clock = Clock::default();
        assert_eq!(clock.nanoseconds(1_000_000), 0);
        clock.publish(1_000_000_000, 1_032_000_000, 100_000_000, 10_000_000);
        assert_eq!(clock.nanoseconds(105_000_000), 995_000_000);
        assert_eq!(clock.nanoseconds(115_000_000), 1_005_000_000);
        assert_eq!(clock.nanoseconds(200_000_000), 1_032_000_000);
        clock.publish(1_032_000_000, 1_064_000_000, 132_000_000, 10_000_000);
        assert_eq!(clock.nanoseconds(137_000_000), 1_027_000_000);
    }
}
