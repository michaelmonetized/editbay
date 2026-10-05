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
