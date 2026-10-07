use crate::{
    Clip, ClipSource, Composition, DocumentCommand, Error, FrameRange, FrameRate, NodeOperation,
    PictureTiming, Project, Result, Sequence, StreamFormat, TimeMap, TimePoint, TimedNode, Track,
    TrackKind,
};
use uuid::Uuid;

/// Create an editable picture sequence from a retained video source.
/// `project`, `source` and `stream` select imported media; `fallback_rate` is the
/// caller's VFR sequence profile. Returns composition-then-sequence commands for
/// one undo group. Uniform indexes use their exact source rate; VFR stays indexed.
/// A final partial source interval occupies the last sequence frame explicitly.
pub fn sequence_from_video(
    project: &Project,
    source: Uuid,
    stream: u32,
    fallback_rate: FrameRate,
) -> Result<Vec<DocumentCommand>> {
    project.validate()?;
    let source = project
        .sources
        .iter()
        .find(|item| item.id == source)
        .ok_or_else(|| Error::Invalid("sequence source is absent".into()))?;
    let stream = source
        .streams
        .iter()
        .find(|item| item.index == stream)
        .ok_or_else(|| Error::Invalid("sequence stream is absent".into()))?;
    let StreamFormat::Video {
        width,
        height,
        sample_aspect,
        timing,
        ..
    } = &stream.format
    else {
        return Err(Error::Invalid(
            "picture sequence requires a video stream".into(),
        ));
    };
    let duration_ticks = stream
        .duration_ticks
        .ok_or_else(|| Error::Invalid("video needs an indexed exclusive end".into()))?;
    let uniform_step = match timing {
        PictureTiming::Constant { rate } => Some(*rate),
        PictureTiming::Variable {
            presentation_ticks,
            end_tick,
        } => {
            let mut intervals = presentation_ticks
                .windows(2)
                .map(|pair| i128::from(pair[1]) - i128::from(pair[0]));
            let step = intervals
                .next()
                .unwrap_or(i128::from(*end_tick) - i128::from(stream.start_tick));
            if step > 0
                && intervals.all(|value| value == step)
                && i128::from(*end_tick)
                    - i128::from(
                        *presentation_ticks
                            .last()
                            .ok_or_else(|| Error::Invalid("empty video index".into()))?,
                    )
                    == step
            {
                let numerator = u128::from(stream.time_base.denominator);
                let denominator = u128::from(stream.time_base.numerator) * step as u128;
                let divisor = gcd(numerator, denominator);
                Some(FrameRate::new(
                    u32::try_from(numerator / divisor).map_err(|_| unsupported())?,
                    u32::try_from(denominator / divisor).map_err(|_| unsupported())?,
                )?)
            } else {
                None
            }
        }
    };
    let rate = uniform_step.unwrap_or(fallback_rate);
    rate.validate()?;
    stream.time_base.validate()?;
    let tick_numerator = u128::from(rate.denominator) * u128::from(stream.time_base.denominator);
    let tick_denominator = u128::from(rate.numerator) * u128::from(stream.time_base.numerator);
    let divisor = gcd(tick_numerator, tick_denominator);
    let step = tick_numerator / divisor;
    let unit = u32::try_from(tick_denominator / divisor).map_err(|_| unsupported())?;
    if step == 0 {
        return Err(unsupported());
    }
    let duration = u64::try_from((u128::from(duration_ticks) * u128::from(unit)).div_ceil(step))
        .map_err(|_| unsupported())?;
    if duration == 0 || duration > i64::MAX as u64 {
        return Err(unsupported());
    }
    let end_tick = i64::try_from(i128::from(stream.start_tick) + i128::from(duration_ticks))
        .map_err(|_| unsupported())?;
    let mut points = vec![TimePoint {
        frame: 0,
        source_tick: i64::try_from(i128::from(stream.start_tick) * i128::from(unit))
            .map_err(|_| unsupported())?,
    }];
    if !(u128::from(duration_ticks) * u128::from(unit)).is_multiple_of(step) && duration > 1 {
        points.push(TimePoint {
            frame: duration - 1,
            source_tick: i64::try_from(
                i128::from(stream.start_tick) * i128::from(unit)
                    + i128::try_from(u128::from(duration - 1) * step).map_err(|_| unsupported())?,
            )
            .map_err(|_| unsupported())?,
        });
    }
    points.push(TimePoint {
        frame: duration,
        source_tick: i64::try_from(i128::from(end_tick) * i128::from(unit))
            .map_err(|_| unsupported())?,
    });
    let width = u32::try_from(
        (u128::from(*width) * u128::from(sample_aspect.numerator))
            .div_ceil(u128::from(sample_aspect.denominator)),
    )
    .map_err(|_| unsupported())?;
    let composition_id = Uuid::new_v4();
    let clip_id = Uuid::new_v4();
    let root = Uuid::new_v4();
    let range = FrameRange {
        start: 0,
        end: duration,
    };
    let name = source.name.clone();
    let composition = Composition {
        id: composition_id,
        name: name.clone(),
        width,
        height: *height,
        frame_rate: rate,
        duration,
        picture: Some(root),
        audio: None,
        tracks: vec![Track {
            id: Uuid::new_v4(),
            name: "Video 1".into(),
            kind: TrackKind::Video,
            enabled: true,
            clips: vec![Clip {
                id: clip_id,
                name: name.clone(),
                range,
                source: ClipSource::Media {
                    source: source.id,
                    stream: stream.index,
                },
                time_map: TimeMap {
                    source_denominator: unit,
                    points,
                },
                linked: None,
            }],
        }],
        nodes: vec![TimedNode {
            id: root,
            range,
            operation: NodeOperation::Source { clip: clip_id },
            animation: vec![],
        }],
    };
    let sequence = Sequence {
        id: project
            .sequences
            .iter()
            .find(|sequence| sequence.composition.is_none())
            .map_or_else(Uuid::new_v4, |sequence| sequence.id),
        name,
        width,
        height: *height,
        frame_rate: rate,
        composition: Some(composition_id),
    };
    Ok(vec![
        DocumentCommand::SetComposition { composition },
        DocumentCommand::SetSequence { sequence },
    ])
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn unsupported() -> Error {
    Error::Invalid("source timing exceeds the exact rational sequence clock range".into())
}

/// Create linked picture and sound at their original source timing.
/// `project`, `source`, `video_stream` and `audio_stream` select imported streams;
/// `fallback_rate` supplies the VFR picture profile. Returns one atomic command
/// group using a nested exact sound clock, preserving delayed starts and tails.
/// Sequence zero is the selected video's origin; earlier sound remains in its
/// nested source composition and later sound extends beyond the last picture.
pub fn sequence_from_video_with_audio(
    project: &Project,
    source: Uuid,
    video_stream: u32,
    audio_stream: u32,
    fallback_rate: FrameRate,
) -> Result<Vec<DocumentCommand>> {
    let mut commands = sequence_from_video(project, source, video_stream, fallback_rate)?;
    let media = project
        .sources
        .iter()
        .find(|item| item.id == source)
        .ok_or_else(|| Error::Invalid("sequence source is absent".into()))?;
    let video = media
        .streams
        .iter()
        .find(|s| s.index == video_stream)
        .ok_or_else(|| Error::Invalid("picture stream is absent".into()))?;
    let audio = media
        .streams
        .iter()
        .find(|s| s.index == audio_stream)
        .ok_or_else(|| Error::Invalid("sound stream is absent".into()))?;
    let StreamFormat::Audio { sample_rate, .. } = audio.format else {
        return Err(Error::Invalid("choose an imported sound stream".into()));
    };
    let DocumentCommand::SetComposition { composition } = &mut commands[0] else {
        return Err(Error::Invalid("picture composition is absent".into()));
    };
    let mut clock = u128::from(composition.frame_rate.numerator);
    for denominator in [
        video.time_base.denominator,
        audio.time_base.denominator,
        sample_rate,
    ] {
        let denominator = u128::from(denominator);
        clock = clock / gcd(clock, denominator) * denominator;
        if clock > u128::from(u32::MAX) {
            return Err(Error::Invalid(
                "source clocks exceed the exact nested sound range".into(),
            ));
        }
    }
    let clock = FrameRate::new(clock as u32, 1)?;
    let picture_origin = clock_tick(video, video.start_tick, clock)?;
    let sound_start = clock_tick(audio, audio.start_tick, clock)?;
    let source_end = i64::try_from(
        i128::from(audio.start_tick)
            + i128::from(audio.duration_ticks.ok_or_else(|| {
                Error::Invalid("sound requires a finite presentation interval".into())
            })?),
    )
    .map_err(|_| unsupported())?;
    let sound_end = clock_tick(audio, source_end, clock)?;
    let origin = picture_origin.min(sound_start);
    let audio_length = i128::from(sound_end) - i128::from(picture_origin);
    if audio_length <= 0 {
        return Err(Error::Invalid(
            "selected sound ends before the picture sequence starts".into(),
        ));
    }
    let ticks_per_frame = u128::from(clock.numerator)
        * u128::from(composition.frame_rate.denominator)
        / u128::from(composition.frame_rate.numerator);
    let sound_frames = u64::try_from((audio_length as u128).div_ceil(ticks_per_frame))
        .map_err(|_| unsupported())?;
    composition.duration = composition.duration.max(sound_frames);
    let offset = u128::try_from(i128::from(picture_origin) - i128::from(origin))
        .map_err(|_| unsupported())?;
    let nested_end = offset
        .checked_add(u128::from(composition.duration) * ticks_per_frame)
        .and_then(|end| i64::try_from(end).ok())
        .ok_or_else(unsupported)?;
    let range = FrameRange {
        start: 0,
        end: composition.duration,
    };
    let nested = Uuid::new_v4();
    let sound_clip = Uuid::new_v4();
    let sound_node = Uuid::new_v4();
    let source_clip = Uuid::new_v4();
    let source_node = Uuid::new_v4();
    let source_range = FrameRange {
        start: u64::try_from(i128::from(sound_start) - i128::from(origin))
            .map_err(|_| unsupported())?,
        end: u64::try_from(i128::from(sound_end) - i128::from(origin))
            .map_err(|_| unsupported())?,
    };
    let nested_duration = (nested_end as u64).max(source_range.end);
    if nested_duration > i64::MAX as u64 {
        return Err(unsupported());
    }
    let picture_clip = &mut composition.tracks[0].clips[0];
    picture_clip.linked = Some(sound_clip);
    let picture_clip_id = picture_clip.id;
    composition.tracks.push(Track {
        id: Uuid::new_v4(),
        name: "Audio 1".into(),
        kind: TrackKind::Audio,
        enabled: true,
        clips: vec![Clip {
            id: sound_clip,
            name: media.name.clone(),
            range,
            source: ClipSource::Composition {
                composition: nested,
            },
            linked: Some(picture_clip_id),
            time_map: TimeMap {
                source_denominator: 1,
                points: vec![
                    TimePoint {
                        frame: 0,
                        source_tick: i64::try_from(offset).map_err(|_| unsupported())?,
                    },
                    TimePoint {
                        frame: composition.duration,
                        source_tick: nested_end,
                    },
                ],
            },
        }],
    });
    composition.nodes.push(TimedNode {
        id: sound_node,
        range,
        operation: NodeOperation::Source { clip: sound_clip },
        animation: vec![],
    });
    composition.audio = Some(sound_node);
    let nested = Composition {
        id: nested,
        name: format!("{} · Original sound", media.name),
        width: composition.width,
        height: composition.height,
        frame_rate: clock,
        duration: nested_duration,
        picture: None,
        audio: Some(source_node),
        tracks: vec![Track {
            id: Uuid::new_v4(),
            name: "Original channels".into(),
            kind: TrackKind::Audio,
            enabled: true,
            clips: vec![Clip {
                id: source_clip,
                name: media.name.clone(),
                range: source_range,
                source: ClipSource::Media {
                    source,
                    stream: audio_stream,
                },
                linked: None,
                time_map: TimeMap {
                    source_denominator: 1,
                    points: vec![
                        TimePoint {
                            frame: 0,
                            source_tick: audio.start_tick,
                        },
                        TimePoint {
                            frame: source_range.end - source_range.start,
                            source_tick: source_end,
                        },
                    ],
                },
            }],
        }],
        nodes: vec![TimedNode {
            id: source_node,
            range: source_range,
            operation: NodeOperation::Source { clip: source_clip },
            animation: vec![],
        }],
    };
    commands.insert(
        0,
        DocumentCommand::SetComposition {
            composition: nested,
        },
    );
    Ok(commands)
}

/// Convert a declared source boundary to a shared integer clock.
/// `stream`, `tick` and `clock` define the units; returns an exact integer tick
/// or an error before any source or document is changed.
fn clock_tick(stream: &crate::SourceStream, tick: i64, clock: FrameRate) -> Result<i64> {
    let position = stream
        .time_base
        .at_rate(crate::SourcePosition::new(tick, 1)?, clock)?;
    if position.denominator != 1 {
        return Err(unsupported());
    }
    Ok(position.numerator)
}

/// Conform an existing composition without changing its elapsed media time.
/// `project` owns `source`; `width`, `height`, `rate` and `name` define delivery.
/// Returns nested composition and sequence commands. A partial final frame is
/// padded with transparent picture and silence after the original media ends.
pub fn conform_sequence(
    project: &Project,
    source: Uuid,
    width: u32,
    height: u32,
    rate: FrameRate,
    name: String,
) -> Result<Vec<DocumentCommand>> {
    project.validate()?;
    let original = project
        .compositions
        .iter()
        .find(|c| c.id == source)
        .ok_or_else(|| Error::Invalid("conform source is absent".into()))?;
    rate.validate()?;
    let numerator = u128::from(original.frame_rate.numerator) * u128::from(rate.denominator);
    let denominator = u128::from(original.frame_rate.denominator) * u128::from(rate.numerator);
    let divisor = gcd(numerator, denominator);
    let step = numerator / divisor;
    let unit = u32::try_from(denominator / divisor).map_err(|_| unsupported())?;
    let duration = u64::try_from((u128::from(original.duration) * u128::from(unit)).div_ceil(step))
        .map_err(|_| unsupported())?;
    let mapped_end = i64::try_from(u128::from(duration) * step).map_err(|_| unsupported())?;
    let padded_duration = u64::try_from((mapped_end as u128).div_ceil(u128::from(unit)))
        .map_err(|_| unsupported())?;
    let clock_id = Uuid::new_v4();
    let mut clock = nested_output(
        original,
        source,
        clock_id,
        original.width,
        original.height,
        original.frame_rate,
        padded_duration,
        original.duration,
        TimeMap {
            source_denominator: 1,
            points: vec![
                TimePoint {
                    frame: 0,
                    source_tick: 0,
                },
                TimePoint {
                    frame: original.duration,
                    source_tick: i64::try_from(original.duration).map_err(|_| unsupported())?,
                },
            ],
        },
        format!("{name} · source clock"),
    );
    clock.duration = padded_duration;
    let id = Uuid::new_v4();
    let composition = nested_output(
        original,
        clock_id,
        id,
        width,
        height,
        rate,
        duration,
        duration,
        TimeMap {
            source_denominator: unit,
            points: vec![
                TimePoint {
                    frame: 0,
                    source_tick: 0,
                },
                TimePoint {
                    frame: duration,
                    source_tick: mapped_end,
                },
            ],
        },
        name.clone(),
    );
    let commands = vec![
        DocumentCommand::SetComposition { composition: clock },
        DocumentCommand::SetComposition { composition },
        DocumentCommand::SetSequence {
            sequence: Sequence {
                id: Uuid::new_v4(),
                name,
                width,
                height,
                frame_rate: rate,
                composition: Some(id),
            },
        },
    ];
    let mut editor = crate::DocumentEditor::new(project.clone())?;
    editor.apply(
        crate::DocumentVersion::of(project),
        "Conform source".into(),
        &commands,
    )?;
    Ok(commands)
}

/// Build paired nested outputs on one exact clock.
/// `original` supplies output sockets; `source`, geometry, ranges and `map`
/// identify the child and timing. Returns a reusable typed composition.
#[allow(clippy::too_many_arguments)]
fn nested_output(
    original: &Composition,
    source: Uuid,
    id: Uuid,
    width: u32,
    height: u32,
    rate: FrameRate,
    duration: u64,
    active: u64,
    map: TimeMap,
    name: String,
) -> Composition {
    let range = FrameRange {
        start: 0,
        end: active,
    };
    let mut composition = Composition {
        id,
        name: name.clone(),
        width,
        height,
        frame_rate: rate,
        duration,
        picture: None,
        audio: None,
        tracks: vec![],
        nodes: vec![],
    };
    let picture_clip = original.picture.map(|_| Uuid::new_v4());
    let sound_clip = original.audio.map(|_| Uuid::new_v4());
    for (kind, clip, linked) in [
        (TrackKind::Video, picture_clip, sound_clip),
        (TrackKind::Audio, sound_clip, picture_clip),
    ] {
        let Some(clip) = clip else { continue };
        let node = Uuid::new_v4();
        composition.tracks.push(Track {
            id: Uuid::new_v4(),
            name: match kind {
                TrackKind::Video => "Picture",
                _ => "Sound",
            }
            .into(),
            kind,
            enabled: true,
            clips: vec![Clip {
                id: clip,
                name: name.clone(),
                range,
                source: ClipSource::Composition {
                    composition: source,
                },
                time_map: map.clone(),
                linked,
            }],
        });
        composition.nodes.push(TimedNode {
            id: node,
            range,
            operation: NodeOperation::Source { clip },
            animation: vec![],
        });
        match kind {
            TrackKind::Video => composition.picture = Some(node),
            _ => composition.audio = Some(node),
        }
    }
    composition
}

/// Create an editable sound source on the project's picture profile.
/// `project`, `source` and `stream` select retained sound; `width`, `height` and
/// `rate` select record geometry. Returns atomic commands with exact sample time
/// and explicit silence in the final partial picture frame.
pub fn sequence_from_audio(
    project: &Project,
    source: Uuid,
    stream: u32,
    width: u32,
    height: u32,
    rate: FrameRate,
) -> Result<Vec<DocumentCommand>> {
    project.validate()?;
    let media = project
        .sources
        .iter()
        .find(|s| s.id == source)
        .ok_or_else(|| Error::Invalid("sound source is absent".into()))?;
    let audio = media
        .streams
        .iter()
        .find(|s| s.index == stream)
        .ok_or_else(|| Error::Invalid("sound stream is absent".into()))?;
    let StreamFormat::Audio { sample_rate, .. } = audio.format else {
        return Err(Error::Invalid("choose an imported sound stream".into()));
    };
    let clock = u128::from(sample_rate)
        / gcd(sample_rate.into(), audio.time_base.denominator.into())
        * u128::from(audio.time_base.denominator);
    let clock = FrameRate::new(u32::try_from(clock).map_err(|_| unsupported())?, 1)?;
    let source_end = i64::try_from(
        i128::from(audio.start_tick)
            + i128::from(
                audio
                    .duration_ticks
                    .ok_or_else(|| Error::Invalid("sound duration is absent".into()))?,
            ),
    )
    .map_err(|_| unsupported())?;
    let duration = u64::try_from(
        i128::from(clock_tick(audio, source_end, clock)?)
            - i128::from(clock_tick(audio, audio.start_tick, clock)?),
    )
    .map_err(|_| unsupported())?;
    let id = Uuid::new_v4();
    let clip = Uuid::new_v4();
    let node = Uuid::new_v4();
    let range = FrameRange {
        start: 0,
        end: duration,
    };
    let composition = Composition {
        id,
        name: format!("{} · exact sound", media.name),
        width,
        height,
        frame_rate: clock,
        duration,
        picture: None,
        audio: Some(node),
        tracks: vec![Track {
            id: Uuid::new_v4(),
            name: "Original sound".into(),
            kind: TrackKind::Audio,
            enabled: true,
            clips: vec![Clip {
                id: clip,
                name: media.name.clone(),
                range,
                source: ClipSource::Media { source, stream },
                linked: None,
                time_map: TimeMap {
                    source_denominator: 1,
                    points: vec![
                        TimePoint {
                            frame: 0,
                            source_tick: audio.start_tick,
                        },
                        TimePoint {
                            frame: duration,
                            source_tick: source_end,
                        },
                    ],
                },
            }],
        }],
        nodes: vec![TimedNode {
            id: node,
            range,
            operation: NodeOperation::Source { clip },
            animation: vec![],
        }],
    };
    let mut staged = project.clone();
    staged.compositions.push(composition.clone());
    let mut commands = conform_sequence(&staged, id, width, height, rate, media.name.clone())?;
    commands.insert(0, DocumentCommand::SetComposition { composition });
    Ok(commands)
}

/// Create an editable title retaining the exact font file.
/// `project` and `profile` supply record geometry; `name`, `text`, `font`, `size`,
/// baseline `position`, linear `rgba` and `duration` define the title. Returns
/// validated asset, composition and sequence commands without changing originals.
#[allow(clippy::too_many_arguments)]
pub(crate) fn title_sequence(
    project: &Project,
    profile: Uuid,
    name: String,
    text: String,
    font: &std::path::Path,
    size: f64,
    position: [f64; 2],
    rgba: [f64; 4],
    duration: u64,
) -> Result<Vec<DocumentCommand>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let original = project
        .compositions
        .iter()
        .find(|c| c.id == profile)
        .ok_or_else(|| Error::Invalid("title profile is absent".into()))?;
    let path = std::fs::canonicalize(font)?;
    let mut bytes = Vec::new();
    std::fs::File::open(&path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
        return Err(Error::Invalid("font exceeds its read budget".into()));
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let retained = project
        .assets
        .iter()
        .find(|a| a.kind == crate::AssetKind::Font && a.path == path && a.sha256 == sha256);
    let asset = retained.map_or_else(Uuid::new_v4, |a| a.id);
    let id = Uuid::new_v4();
    let root = Uuid::new_v4();
    let mut commands = Vec::new();
    if retained.is_none() {
        commands.push(DocumentCommand::SetAsset {
            asset: crate::AssetReference {
                id: asset,
                kind: crate::AssetKind::Font,
                path,
                sha256,
                bytes: bytes.len() as u64,
                provenance: "Original title font retained without modification".into(),
            },
        });
    }
    commands.push(DocumentCommand::SetComposition {
        composition: Composition {
            id,
            name: name.clone(),
            width: original.width,
            height: original.height,
            frame_rate: original.frame_rate,
            duration,
            picture: Some(root),
            audio: None,
            tracks: vec![],
            nodes: vec![TimedNode {
                id: root,
                range: FrameRange {
                    start: 0,
                    end: duration,
                },
                operation: NodeOperation::Text {
                    text,
                    font: asset,
                    size,
                    position,
                    rgba,
                },
                animation: vec![],
            }],
        },
    });
    commands.push(DocumentCommand::SetSequence {
        sequence: Sequence {
            id: Uuid::new_v4(),
            name,
            width: original.width,
            height: original.height,
            frame_rate: original.frame_rate,
            composition: Some(id),
        },
    });
    let mut editor = crate::DocumentEditor::new(project.clone())?;
    editor.apply(
        crate::DocumentVersion::of(project),
        "Create title".into(),
        &commands,
    )?;
    Ok(commands)
}

/// Create reusable picture and sound fades that survive exact source trims.
/// `project`, `source` and `name` select the child; `fade_in` and `fade_out` are
/// frame lengths. Returns shared animated graph commands with original timing.
pub(crate) fn faded_sequence(
    project: &Project,
    source: Uuid,
    name: String,
    fade_in: u64,
    fade_out: u64,
) -> Result<Vec<DocumentCommand>> {
    let original = project
        .compositions
        .iter()
        .find(|c| c.id == source)
        .ok_or_else(|| Error::Invalid("fade source is absent".into()))?;
    if fade_in
        .checked_add(fade_out)
        .is_none_or(|sum| sum > original.duration)
    {
        return Err(Error::Invalid("fades exceed source duration".into()));
    }
    let id = Uuid::new_v4();
    let mut composition = nested_output(
        original,
        source,
        id,
        original.width,
        original.height,
        original.frame_rate,
        original.duration,
        original.duration,
        TimeMap {
            source_denominator: 1,
            points: vec![
                TimePoint {
                    frame: 0,
                    source_tick: 0,
                },
                TimePoint {
                    frame: original.duration,
                    source_tick: i64::try_from(original.duration).map_err(|_| unsupported())?,
                },
            ],
        },
        name.clone(),
    );
    let range = FrameRange {
        start: 0,
        end: original.duration,
    };
    for audio in [false, true] {
        let output = if audio {
            &mut composition.audio
        } else {
            &mut composition.picture
        };
        let Some(input) = *output else { continue };
        let root = Uuid::new_v4();
        let mut keys = std::collections::BTreeMap::new();
        keys.insert(0, if fade_in > 0 { 0. } else { 1. });
        if fade_in > 0 {
            keys.insert(fade_in, 1.);
        }
        if fade_out > 0 {
            keys.insert(original.duration - fade_out, 1.);
        }
        keys.insert(original.duration, if fade_out > 0 { 0. } else { 1. });
        composition.nodes.push(TimedNode {
            id: root,
            range,
            operation: if audio {
                NodeOperation::Gain {
                    audio: input,
                    gain: 1.,
                }
            } else {
                NodeOperation::Transform {
                    image: input,
                    translation: [0.; 2],
                    scale: [1.; 2],
                    rotation: 0.,
                    opacity: 1.,
                }
            },
            animation: vec![crate::AnimationChannel {
                id: Uuid::new_v4(),
                property: if audio {
                    crate::AnimatedProperty::Gain
                } else {
                    crate::AnimatedProperty::Opacity
                },
                interpolation: crate::Interpolation::Linear,
                keys: keys
                    .into_iter()
                    .map(|(frame, value)| crate::Keyframe {
                        frame,
                        value,
                        in_tangent: 0.,
                        out_tangent: 0.,
                    })
                    .collect(),
            }],
        });
        *output = Some(root);
    }
    let commands = vec![
        DocumentCommand::SetComposition { composition },
        DocumentCommand::SetSequence {
            sequence: Sequence {
                id: Uuid::new_v4(),
                name,
                width: original.width,
                height: original.height,
                frame_rate: original.frame_rate,
                composition: Some(id),
            },
        },
    ];
    let mut editor = crate::DocumentEditor::new(project.clone())?;
    editor.apply(
        crate::DocumentVersion::of(project),
        "Create fades".into(),
        &commands,
    )?;
    Ok(commands)
}
