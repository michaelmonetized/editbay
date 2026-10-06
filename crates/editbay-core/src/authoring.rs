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
    if !tick_numerator.is_multiple_of(tick_denominator) {
        return Err(unsupported());
    }
    let step = tick_numerator / tick_denominator;
    if step == 0 {
        return Err(unsupported());
    }
    let duration =
        u64::try_from(u128::from(duration_ticks).div_ceil(step)).map_err(|_| unsupported())?;
    if duration == 0 || duration > i64::MAX as u64 {
        return Err(unsupported());
    }
    let end_tick = i64::try_from(i128::from(stream.start_tick) + i128::from(duration_ticks))
        .map_err(|_| unsupported())?;
    let mut points = vec![TimePoint {
        frame: 0,
        source_tick: stream.start_tick,
    }];
    if !u128::from(duration_ticks).is_multiple_of(step) && duration > 1 {
        points.push(TimePoint {
            frame: duration - 1,
            source_tick: i64::try_from(
                i128::from(stream.start_tick)
                    + i128::try_from(u128::from(duration - 1) * step).map_err(|_| unsupported())?,
            )
            .map_err(|_| unsupported())?,
        });
    }
    points.push(TimePoint {
        frame: duration,
        source_tick: end_tick,
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
                time_map: TimeMap { points },
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
    Error::Invalid("source timing cannot be represented by this sequence profile; choose a rate with whole source ticks per frame".into())
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
