use crate::{DeliveryFormat, DeliveryRequest, Phase, Progress, Receipt, Result, SignalComparison};
use editbay_audio::{SoundPreparation, SoundRenderBudget, SoundRenderer};
use editbay_core::{
    DocumentVersion, EvaluationSnapshot, OutputTransfer, Project, SoundBudget, SoundSnapshot,
    SourceColor, SourcePosition, TimeBase, WorkingGamut,
};
use editbay_media::{
    Cancellation, LosslessMovProfile, LosslessMovWriter, NativeAudioReader, PcmBudget,
    PictureBudget, SourceFile, VideoReader,
};
use editbay_render::{GraphBudget, GraphRenderer, ImageBoundary};
use sha2::{Digest, Sha256};
use std::{fs::File, os::unix::fs::MetadataExt, sync::Arc};

pub(crate) struct Session {
    version: DocumentVersion,
    request: DeliveryRequest,
    profile: LosslessMovProfile,
    cancel: Cancellation,
    picture: GraphRenderer,
    sound: SoundRenderer,
    plan: Arc<SoundSnapshot>,
    writer: Option<LosslessMovWriter>,
    file: File,
    phase: Phase,
    pictures: u64,
    samples: u64,
    first_sample: u64,
    total_samples: u64,
    source_preparation: SoundPreparation,
    expected_pixels: Sha256,
    expected_pcm: Sha256,
    decoded: Sha256,
    video: Option<VideoReader>,
    audio: Option<NativeAudioReader>,
    source: Option<SourceFile>,
    clipped: u64,
    receipt: Option<Receipt>,
    decoded_pixels: Option<String>,
    picture_quality: SignalComparison,
    sound_quality: SignalComparison,
    decoded_padding: u64,
}

impl Session {
    pub(crate) fn new(project: Arc<Project>, request: DeliveryRequest, file: File) -> Result<Self> {
        if project.color.output_gamut != WorkingGamut::Bt709
            || project.color.output_transfer != OutputTransfer::Srgb
        {
            return Err(
                "The PNG/float PCM master requires Rec.709 primaries and sRGB output".into(),
            );
        }
        let owned = file.metadata()?;
        if !owned.is_file() || owned.nlink() != 0 {
            return Err("Delivery requires an anonymous private output file".into());
        }
        let version = DocumentVersion::of(&project);
        let evaluation = Arc::new(EvaluationSnapshot::new(project.clone())?);
        let root = project
            .compositions
            .iter()
            .find(|c| c.id == request.composition)
            .ok_or("Delivery composition is absent")?;
        let plan = Arc::new(SoundSnapshot::at_output_rate(
            evaluation.clone(),
            request.composition,
            request.sample_rate,
            SoundBudget::default(),
        )?);
        let range = request.frame_range(root.duration)?;
        let profile = LosslessMovProfile {
            width: root.width,
            height: root.height,
            frame_rate: root.frame_rate,
            first_frame: range.start,
            frames: range.end - range.start,
            sample_rate: request.sample_rate,
            channels: plan.profile().channels.clone(),
        };
        request.format.validate(&profile)?;
        let first_sample = profile.sample_origin()?;
        let total_samples = profile.samples_through(profile.frames)?;
        let cancel = Cancellation::new()?;
        let picture = GraphRenderer::new(
            evaluation,
            PictureBudget::default(),
            GraphBudget::default(),
            cancel.clone(),
        )?;
        let sound = SoundRenderer::new(
            plan.clone(),
            PcmBudget::default(),
            SoundRenderBudget::default(),
            cancel.clone(),
        )?;
        let source_preparation = sound.source_preparation(first_sample, total_samples)?;
        let phase = if source_preparation.progress().ready {
            Phase::Rendering
        } else {
            Phase::Preparing
        };
        let writer = Some(LosslessMovWriter::encoded(
            &file,
            profile.clone(),
            request.format,
            cancel.clone(),
        )?);
        Ok(Self {
            version,
            request,
            profile,
            cancel,
            picture,
            sound,
            plan,
            writer,
            file,
            phase,
            pictures: 0,
            samples: 0,
            first_sample,
            total_samples,
            source_preparation,
            expected_pixels: Sha256::new(),
            expected_pcm: Sha256::new(),
            decoded: Sha256::new(),
            video: None,
            audio: None,
            source: None,
            clipped: 0,
            receipt: None,
            decoded_pixels: None,
            picture_quality: SignalComparison::default(),
            sound_quality: SignalComparison::default(),
            decoded_padding: 0,
        })
    }

    pub(crate) fn progress(&self) -> Progress {
        Progress {
            phase: self.phase,
            prepared_samples: self.source_preparation.progress().prepared_samples,
            total_preparation_samples: self.source_preparation.progress().total_samples,
            pictures: self.pictures,
            samples: self.samples.min(self.total_samples),
            total_pictures: self.profile.frames,
            total_samples: self.total_samples,
        }
    }

    pub(crate) fn receipt(&self) -> Option<&Receipt> {
        self.receipt.as_ref()
    }

    pub(crate) fn step(&mut self) -> Result<()> {
        match self.phase {
            Phase::Rendering => self.render(),
            Phase::VerifyingPictures => self.verify_picture(),
            Phase::VerifyingSound => self.verify_sound(),
            Phase::Complete => Err("Delivery is already complete".into()),
            Phase::Preparing => self.prepare_sound(),
            Phase::VerifyingFile | Phase::Publishing => {
                Err("File publication belongs to the parent".into())
            }
        }
    }

    fn prepare_sound(&mut self) -> Result<()> {
        if self
            .sound
            .prepare_sources_step(&mut self.source_preparation)?
            .ready
        {
            self.phase = Phase::Rendering;
        }
        Ok(())
    }

    fn render(&mut self) -> Result<()> {
        let working = self.picture.render(
            self.request.composition,
            SourcePosition {
                numerator: i64::try_from(self.profile.first_frame + self.pictures)?,
                denominator: 1,
            },
            false,
        )?;
        let output = self.picture.convert(&working, ImageBoundary::Output)?;
        let (mut rgba, clipped) = rgba8(&self.picture.readback(&output)?)?;
        flatten(&mut rgba, self.request.format);
        self.clipped += clipped;
        let writer = self.writer.as_mut().ok_or("Delivery encoder is absent")?;
        writer.picture(self.pictures, &rgba)?;
        self.expected_pixels.update(&rgba);
        let end = self.profile.samples_through(self.pictures + 1)?;
        while self.samples < end {
            let count = (end - self.samples).min(4096) as u32;
            let plan = self.plan.prepare(self.first_sample + self.samples, count)?;
            let sound = self.sound.render(&plan)?;
            self.sound.validate_result(&sound)?;
            if self.request.format == DeliveryFormat::H264Mp4
                && sound
                    .sound()
                    .samples()
                    .iter()
                    .any(|sample| sample.abs() > 1.)
            {
                return Err(
                    "AAC delivery exceeds full scale; lower the sound mix before exporting".into(),
                );
            }
            writer.sound(self.samples, sound.sound().samples())?;
            for sample in sound.sound().samples() {
                self.expected_pcm.update(sample.to_le_bytes());
            }
            self.samples += u64::from(count);
        }
        self.pictures += 1;
        if self.pictures == self.profile.frames {
            self.writer
                .take()
                .ok_or("Delivery encoder is absent")?
                .finish()?;
            self.file.sync_all()?;
            self.picture.verify_sources()?;
            self.sound.verify_sources()?;
            let source = SourceFile::private(&self.file, &self.cancel)?;
            let probe = source.probe(self.cancel.clone())?;
            if probe.streams.len() != 2 {
                return Err("Delivery does not contain exactly picture and sound".into());
            }
            let video = &probe.streams[0];
            let audio = &probe.streams[1];
            let (video_codec, audio_codec, matrix, range) = match self.request.format {
                DeliveryFormat::LosslessMov => ("png", "pcm_f32le", 0, 2),
                DeliveryFormat::H264Mp4 => ("h264", "aac", 1, 1),
                DeliveryFormat::ProresMov => ("prores", "pcm_f32le", 1, 1),
            };
            if video.codec != video_codec
                || video.width != Some(self.profile.width)
                || video.height != Some(self.profile.height)
                || video.color
                    != (SourceColor {
                        primaries: 1,
                        transfer: 13,
                        matrix,
                        range,
                    })
                || video.time_base
                    != Some(TimeBase {
                        numerator: 1,
                        denominator: self.profile.frame_rate.numerator,
                    })
                || audio.codec != audio_codec
                || audio.sample_rate != Some(self.profile.sample_rate)
                || audio.channels != self.profile.channels
                || audio.time_base
                    != Some(TimeBase {
                        numerator: 1,
                        denominator: self.profile.sample_rate,
                    })
            {
                return Err("Delivery metadata differs from the requested profile".into());
            }
            self.video = Some(VideoReader::open_stream(&source, 0, self.cancel.clone())?);
            self.source = Some(source);
            self.pictures = 0;
            self.samples = 0;
            self.phase = Phase::VerifyingPictures;
        }
        Ok(())
    }

    fn verify_picture(&mut self) -> Result<()> {
        let reader = self
            .video
            .as_mut()
            .ok_or("Delivery picture verifier is absent")?;
        if let Some(frame) = reader.next_frame()? {
            if self.pictures >= self.profile.frames
                || frame.source_tick
                    != Some(i64::try_from(
                        u128::from(self.pictures) * u128::from(self.profile.frame_rate.denominator),
                    )?)
            {
                return Err("Delivered pictures have the wrong time or count".into());
            }
            self.decoded.update(&frame.rgba);
            if self.request.format != DeliveryFormat::LosslessMov {
                let working = self.picture.render(
                    self.request.composition,
                    SourcePosition {
                        numerator: i64::try_from(self.profile.first_frame + self.pictures)?,
                        denominator: 1,
                    },
                    false,
                )?;
                let output = self.picture.convert(&working, ImageBoundary::Output)?;
                let (mut reference, _) = rgba8(&self.picture.readback(&output)?)?;
                flatten(&mut reference, self.request.format);
                if reference.len() != frame.rgba.len() {
                    return Err("Decoded delivery picture has the wrong geometry".into());
                }
                for (expected, actual) in reference.iter().zip(&frame.rgba) {
                    self.picture_quality
                        .observe(f64::from(*expected) / 255., f64::from(*actual) / 255.)?;
                }
            }
            self.pictures += 1;
        } else {
            if self.pictures != self.profile.frames
                || (self.request.format == DeliveryFormat::LosslessMov
                    && self.decoded.clone().finalize() != self.expected_pixels.clone().finalize())
            {
                return Err("Decoded delivery pictures differ from shared graph output".into());
            }
            if self.request.format != DeliveryFormat::LosslessMov {
                let limit = if self.request.format == DeliveryFormat::H264Mp4 {
                    0.035
                } else {
                    0.015
                };
                if self
                    .picture_quality
                    .rms_error()
                    .is_none_or(|error| error > limit)
                {
                    return Err(
                        "Compressed delivery exceeds its declared picture quality tolerance".into(),
                    );
                }
            }
            self.decoded_pixels = Some(format!("{:x}", self.decoded.clone().finalize()));
            self.video = None;
            self.audio = Some(NativeAudioReader::open_stream(
                self.source.as_ref().ok_or("Delivery source is absent")?,
                1,
                self.cancel.clone(),
            )?);
            self.decoded = Sha256::new();
            self.phase = Phase::VerifyingSound;
        }
        Ok(())
    }

    fn verify_sound(&mut self) -> Result<()> {
        let reader = self
            .audio
            .as_mut()
            .ok_or("Delivery sound verifier is absent")?;
        if let Some(block) = reader.next_block()? {
            if block.first_sample != Some(i64::try_from(self.samples)?) {
                return Err("Delivered sound is discontinuous".into());
            }
            let channels = self.profile.channels.len();
            let frames = (block.samples.len() / channels) as u64;
            let valid = frames.min(self.total_samples.saturating_sub(self.samples));
            let padding = frames - valid;
            self.decoded_padding += padding;
            if self.decoded_padding
                > if self.request.format == DeliveryFormat::H264Mp4 {
                    1023
                } else {
                    0
                }
            {
                return Err("Delivery contains excess sound beyond codec padding".into());
            }
            let valid_samples = &block.samples[..valid as usize * channels];
            if self.request.format == DeliveryFormat::H264Mp4 {
                let mut first = 0usize;
                while first < valid as usize {
                    let count = (valid as usize - first).min(4096) as u32;
                    let plan = self
                        .plan
                        .prepare(self.first_sample + self.samples + first as u64, count)?;
                    let reference = self.sound.render(&plan)?;
                    self.sound.validate_result(&reference)?;
                    for (expected, actual) in
                        reference.sound().samples().iter().zip(
                            &valid_samples[first * channels..(first + count as usize) * channels],
                        )
                    {
                        self.sound_quality
                            .observe(f64::from(*expected), f64::from(*actual))?;
                    }
                    first += count as usize;
                }
            }
            self.samples += frames;
            for sample in valid_samples {
                self.decoded.update(sample.to_le_bytes());
            }
        } else {
            if self.samples - self.decoded_padding != self.total_samples
                || (self.request.format != DeliveryFormat::H264Mp4
                    && self.decoded.clone().finalize() != self.expected_pcm.clone().finalize())
            {
                return Err("Decoded delivery sound differs from shared graph output".into());
            }
            if self.request.format == DeliveryFormat::H264Mp4
                && self
                    .sound_quality
                    .rms_error()
                    .is_none_or(|error| error > 0.02)
            {
                return Err("AAC delivery exceeds its declared sound quality tolerance".into());
            }
            self.picture.verify_sources()?;
            self.sound.verify_sources()?;
            let source = self.source.as_ref().ok_or("Delivery source is absent")?;
            source.verify(&self.cancel)?;
            self.receipt = Some(Receipt {
                format: self.request.format,
                version: self.version,
                profile: self.profile.clone(),
                pixel_sha256: format!("{:x}", self.expected_pixels.clone().finalize()),
                pcm_sha256: format!("{:x}", self.expected_pcm.clone().finalize()),
                decoded_pixel_sha256: self
                    .decoded_pixels
                    .clone()
                    .ok_or("Missing picture QC hash")?,
                decoded_pcm_sha256: format!("{:x}", self.decoded.clone().finalize()),
                picture_quality: self.picture_quality.clone(),
                sound_quality: self.sound_quality.clone(),
                file_sha256: source.fingerprint().sha256.clone(),
                file_bytes: source.fingerprint().bytes,
                clipped_picture_values: self.clipped,
                adapter: format!("{:?}", self.picture.adapter),
            });
            self.phase = Phase::Complete;
        }
        Ok(())
    }
}

fn flatten(rgba: &mut [u8], format: DeliveryFormat) {
    if format == DeliveryFormat::H264Mp4 {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            for value in &mut pixel[..3] {
                *value = ((u32::from(*value) * alpha + 127) / 255) as u8;
            }
            pixel[3] = 255;
        }
    }
}

fn rgba8(premultiplied: &[f32]) -> Result<(Vec<u8>, u64)> {
    if !premultiplied.len().is_multiple_of(4) {
        return Err("Malformed rendered picture".into());
    }
    let mut output = Vec::with_capacity(premultiplied.len());
    let mut clipped = 0;
    for pixel in premultiplied.as_chunks::<4>().0 {
        let alpha = pixel[3];
        if pixel.iter().any(|v| !v.is_finite()) || !(0. ..=1.).contains(&alpha) {
            return Err("Rendered picture contains invalid color or alpha".into());
        }
        for value in &pixel[..3] {
            let straight = if alpha == 0. { 0. } else { *value / alpha };
            clipped += u64::from(!(0. ..=1.).contains(&straight));
            output.push((straight.clamp(0., 1.) * 255.).round() as u8);
        }
        output.push((alpha * 255.).round() as u8);
    }
    Ok((output, clipped))
}
