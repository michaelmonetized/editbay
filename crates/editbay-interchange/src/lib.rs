//! Editorial formats connected to EditBay's retained sources and native graph.
use editbay_core as eb;
pub use filmcraft_interchange::{ExportOptions, Format, Report};
use filmcraft_project as fp;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};
use uuid::Uuid;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImportOptions {
    pub base_dir: Option<String>,
    pub edl_frame_rate: Option<eb::FrameRate>,
    pub relinks: BTreeMap<String, PathBuf>,
}

/// Read a bounded editorial document and probe its linked originals.
/// `bytes`, `format` and `options` select interchange and explicit relinks; `cancel`
/// interrupts native source work. Returns a validated editable project and loss report.
pub fn import_document(
    bytes: &[u8],
    format: Format,
    options: &ImportOptions,
    cancel: &editbay_media::Cancellation,
) -> Result<(eb::Project, Report)> {
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("Editorial document exceeds 32 MiB".into());
    }
    let (imported, mut report) = filmcraft_interchange::import_with(
        bytes,
        format,
        &filmcraft_interchange::ImportOptions {
            base_dir: options.base_dir.clone(),
            edl_frame_rate: options.edl_frame_rate.map(rate_out),
            name: None,
        },
    )?;
    let mut project = eb::Project::new(imported.project.name.clone())?;
    let mut media = HashMap::new();
    for (id, item) in &imported.project.items {
        if let fp::ItemKind::Media(m) = &item.kind {
            let fp::MediaRef::File { path } = &m.media else {
                report.warn(format!("Synthetic media {} is not imported", item.name));
                continue;
            };
            let path = options
                .relinks
                .get(path)
                .cloned()
                .unwrap_or_else(|| PathBuf::from(path));
            let source = editbay_media::SourceFile::open(&path, cancel)?;
            let probe = source.probe(cancel.clone())?;
            let streams: Vec<_> = probe
                .streams
                .iter()
                .filter(|s| s.decoder_available)
                .map(|s| s.index)
                .collect();
            let ingested = source.ingest(item.name.clone(), &streams, cancel.clone(), |_, _| {})?;
            media.insert(*id, ingested.source.id);
            project.assets.push(ingested.asset);
            project.sources.push(ingested.source);
        }
    }
    let sequence_ids: HashMap<_, _> = imported
        .project
        .items
        .iter()
        .filter(|(_, i)| matches!(i.kind, fp::ItemKind::Sequence(_)))
        .map(|(id, _)| (*id, Uuid::new_v4()))
        .collect();
    project.sequences.clear();
    for (id, item) in &imported.project.items {
        let fp::ItemKind::Sequence(sequence) = &item.kind else {
            continue;
        };
        let rate = eb::FrameRate::new(
            sequence.settings.frame_rate.num.try_into()?,
            sequence.settings.frame_rate.den.try_into()?,
        )?;
        let duration = frames(sequence.duration(), rate)?.max(1);
        let composition = sequence_ids[id];
        let mut scene = eb::Composition {
            id: composition,
            name: item.name.clone(),
            width: sequence.settings.width,
            height: sequence.settings.height,
            frame_rate: rate,
            duration,
            tracks: vec![],
            nodes: vec![],
            picture: None,
            audio: None,
        };
        let mut clip_ids = HashMap::new();
        let mut links: HashMap<u64, Vec<Uuid>> = HashMap::new();
        for track in sequence.video_tracks.iter().chain(&sequence.audio_tracks) {
            let kind = if track.kind == fp::TrackKind::Video {
                eb::TrackKind::Video
            } else {
                eb::TrackKind::Audio
            };
            let mut out = eb::Track {
                id: Uuid::new_v4(),
                name: track.name.clone(),
                kind,
                enabled: track.enabled && !track.muted,
                clips: vec![],
            };
            for c in &track.items {
                if !c.enabled {
                    report.info(format!("Disabled clip {} omitted", c.name));
                    continue;
                }
                let (source, base, origin) = if let Some(source) = media.get(&c.item) {
                    let s = project
                        .sources
                        .iter()
                        .find(|s| s.id == *source)
                        .ok_or("Imported source absent")?;
                    let stream = if kind == eb::TrackKind::Video {
                        s.streams
                            .iter()
                            .find(|s| matches!(s.format, eb::StreamFormat::Video { .. }))
                    } else {
                        s.streams
                            .iter()
                            .filter(|s| matches!(s.format, eb::StreamFormat::Audio { .. }))
                            .nth(c.audio_stream)
                    }
                    .ok_or("Editorial clip has no matching native source stream")?;
                    (
                        eb::ClipSource::Media {
                            source: *source,
                            stream: stream.index,
                        },
                        stream.time_base,
                        stream.start_tick,
                    )
                } else if let Some(child) = sequence_ids.get(&c.item) {
                    let child = imported
                        .project
                        .sequence(c.item)
                        .ok_or("Nested sequence absent")?;
                    (
                        eb::ClipSource::Composition {
                            composition: sequence_ids[&c.item],
                        },
                        eb::TimeBase {
                            numerator: child.settings.frame_rate.den.try_into()?,
                            denominator: child.settings.frame_rate.num.try_into()?,
                        },
                        0,
                    )
                } else {
                    return Err(
                        format!("Clip {} references unsupported or missing media", c.name).into(),
                    );
                };
                let start = frames(c.start, rate)?;
                let end = frames(c.end(), rate)?;
                if end <= start {
                    return Err("Editorial clip has no whole-frame duration".into());
                }
                let mut range = eb::FrameRange { start, end };
                for t in &track.transitions {
                    if t.to == Some(c.id) {
                        range.start = range.start.min(frames(t.start, rate)?);
                    }
                    if t.from == Some(c.id) {
                        range.end = range.end.max(frames(t.end(), rate)?);
                    }
                }
                scene.duration = scene.duration.max(range.end);
                let speed = if c.reverse { -c.speed.abs() } else { c.speed };
                let (sn, sd) = decimal(speed)?;
                let nominal = if let Some(hold) = c.frame_hold {
                    eb::SourcePosition::new(hold.0, 1)?
                } else if c.reverse {
                    fraction(
                        i128::from(c.source_in.0) * i128::from(sd)
                            + i128::from(c.duration.0) * i128::from(sn.unsigned_abs()),
                        sd,
                    )?
                } else {
                    eb::SourcePosition::new(c.source_in.0, 1)?
                };
                let mut positions = Vec::new();
                for frame in [range.start, range.end] {
                    let offset = i128::from(frame) - i128::from(start);
                    let delta = if c.frame_hold.is_some() {
                        eb::SourcePosition::new(0, 1)?
                    } else {
                        fraction(
                            offset
                                * i128::from(rate.denominator)
                                * i128::from(TICKS_PER_SECOND)
                                * i128::from(sn),
                            u64::from(rate.numerator)
                                .checked_mul(sd)
                                .ok_or("Speed denominator overflow")?,
                        )?
                    };
                    let ticks = add(nominal, delta)?;
                    positions.push(add(
                        fraction(
                            i128::from(ticks.numerator) * i128::from(base.denominator),
                            ticks
                                .denominator
                                .checked_mul(TICKS_PER_SECOND as u64)
                                .and_then(|v| v.checked_mul(u64::from(base.numerator)))
                                .ok_or("Source denominator overflow")?,
                        )?,
                        eb::SourcePosition::new(origin, 1)?,
                    )?);
                }
                let map = time_map(range.end - range.start, positions[0], positions[1])?;
                let native = Uuid::new_v4();
                clip_ids.insert(c.id, native);
                if let Some(link) = c.link {
                    links.entry(link).or_default().push(native);
                }
                out.clips.push(eb::Clip {
                    id: native,
                    name: c.name.clone(),
                    range,
                    source,
                    time_map: map,
                    linked: None,
                });
                if c.effects
                    .iter()
                    .any(|e| e.enabled && e.def().is_none_or(|d| !d.intrinsic))
                    || c.gain_db != 0.
                {
                    report.warn(format!("Clip {} effects/gain are not mapped", c.name));
                }
            }
            for transition in &track.transitions {
                let start = frames(transition.start, rate)?;
                let end = frames(transition.end(), rate)?;
                let from = transition.from.and_then(|id| clip_ids.get(&id).copied());
                let to = transition.to.and_then(|id| clip_ids.get(&id).copied());
                let cut = transition
                    .to
                    .and_then(|id| track.item(id))
                    .map(|c| frames(c.start, rate))
                    .transpose()?
                    .unwrap_or(end);
                let effect = transition.effect.effect.as_str();
                if !matches!(
                    effect,
                    "cross_dissolve"
                        | "crossDissolve"
                        | "cross-dissolve"
                        | "crossfade"
                        | "constant_power"
                        | "constant_gain"
                ) {
                    report.warn(format!("Transition {effect} mapped to a linear dissolve"));
                }
                project.transitions.push(eb::EditorialTransition {
                    id: Uuid::new_v4(),
                    composition,
                    range: eb::FrameRange { start, end },
                    cut,
                    from,
                    to,
                });
            }
            scene.tracks.push(out);
        }
        for pair in links.values() {
            if pair.len() != 2 {
                report.warn("Only reciprocal picture/sound pairs can be linked");
                continue;
            }
            for track in &mut scene.tracks {
                for c in &mut track.clips {
                    if c.id == pair[0] {
                        c.linked = Some(pair[1]);
                    } else if c.id == pair[1] {
                        c.linked = Some(pair[0]);
                    }
                }
            }
        }
        eb::rebuild_editorial(&mut scene, &project.transitions)?;
        project.compositions.push(scene);
        if imported.sequences.contains(id) {
            project.sequences.push(eb::Sequence {
                id: Uuid::new_v4(),
                name: item.name.clone(),
                width: sequence.settings.width,
                height: sequence.settings.height,
                frame_rate: rate,
                composition: Some(composition),
            });
        }
    }
    if project.sequences.is_empty() {
        return Err("Interchange contains no top-level sequence".into());
    }
    project.validate()?;
    Ok((project, report))
}

/// Export real editorial bytes with explicit format losses.
/// `project`, `composition`, `format` and `options` select the native cut and writer.
/// Returns independent-format bytes and a report; originals are never opened for writing.
pub fn export_document(
    project: &eb::Project,
    composition: Uuid,
    format: Format,
    options: &ExportOptions,
) -> Result<(Vec<u8>, Report)> {
    project.validate()?;
    let (external, ids, mut report) = to_external(project)?;
    let id = *ids.get(&composition).ok_or("Export composition absent")?;
    let (bytes, loss) = filmcraft_interchange::export(&external, id, format, options)?;
    report.entries.extend(loss.entries);
    Ok((bytes, report))
}

/// Verify a moved original and prepare a shared relink command.
/// `project` owns `asset`; `path` must have the same complete-byte fingerprint.
/// Returns a command preserving source identities, stream timing and all edits.
pub fn relink(
    project: &eb::Project,
    asset: Uuid,
    path: &Path,
    cancel: &editbay_media::Cancellation,
) -> Result<eb::DocumentCommand> {
    let mut reference = project
        .assets
        .iter()
        .find(|a| a.id == asset)
        .ok_or("Relink asset absent")?
        .clone();
    let file = editbay_media::SourceFile::open(path, cancel)?;
    if file.fingerprint().sha256 != reference.sha256 || file.fingerprint().bytes != reference.bytes
    {
        return Err("Relink must match the original complete-byte fingerprint".into());
    }
    reference.path = file.path().to_path_buf();
    Ok(eb::DocumentCommand::SetAsset { asset: reference })
}

fn to_external(project: &eb::Project) -> Result<(fp::Project, HashMap<Uuid, fp::ItemId>, Report)> {
    let mut out = fp::Project::new(&project.name);
    let mut report = Report::default();
    let mut ids = HashMap::new();
    for source in &project.sources {
        let asset = project
            .assets
            .iter()
            .find(|a| a.id == source.asset)
            .ok_or("Source asset absent")?;
        let mut video = None;
        let mut audio = vec![];
        let mut duration = Tick::ZERO;
        for s in &source.streams {
            if let Some(d) = s.duration_ticks {
                duration = duration.max(tick(
                    eb::SourcePosition::new(d.try_into()?, 1)?,
                    s.time_base,
                )?);
            }
            match &s.format {
                eb::StreamFormat::Video {
                    width,
                    height,
                    sample_aspect,
                    timing,
                    alpha,
                    ..
                } => {
                    let rate = match timing {
                        eb::PictureTiming::Constant { rate } => *rate,
                        _ => project.sequences[0].frame_rate,
                    };
                    video = Some(filmcraft_media::VideoStreamInfo {
                        width: *width,
                        height: *height,
                        frame_rate: rate_out(rate),
                        par: (sample_aspect.numerator, sample_aspect.denominator),
                        codec: s.codec.clone(),
                        pixel_format: String::new(),
                        color: filmcraft_color::ColorInfo::SRGB_FULL,
                        has_alpha: *alpha != eb::AlphaMode::Opaque,
                        bitrate: None,
                        hdr: None,
                    });
                }
                eb::StreamFormat::Audio {
                    sample_rate,
                    channels,
                } => audio.push(filmcraft_media::AudioStreamInfo {
                    sample_rate: *sample_rate,
                    channels: channels.len().try_into()?,
                    codec: s.codec.clone(),
                    bits_per_sample: None,
                }),
            }
        }
        let info = filmcraft_media::MediaInfo {
            name: source.name.clone(),
            kind: if video.is_some() {
                filmcraft_media::MediaKind::Movie
            } else {
                filmcraft_media::MediaKind::AudioOnly
            },
            duration,
            video,
            audio_streams: audio,
            container: String::new(),
            start_timecode: None,
            file_size: Some(asset.bytes),
        };
        let m = fp::MediaClip {
            media: fp::MediaRef::File {
                path: asset
                    .path
                    .to_str()
                    .ok_or("Interchange media paths require UTF-8")?
                    .into(),
            },
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        };
        ids.insert(
            source.id,
            out.add_item(&source.name, fp::Label::Iris, fp::ItemKind::Media(m), None),
        );
    }
    for scene in &project.compositions {
        let id = out.new_sequence(
            &scene.name,
            fp::SequenceSettings {
                width: scene.width,
                height: scene.height,
                frame_rate: rate_out(scene.frame_rate),
                ..Default::default()
            },
            0,
            0,
            None,
        );
        ids.insert(scene.id, id);
    }
    let mut native_clips = HashMap::new();
    for scene in &project.compositions {
        let mut tracks = vec![];
        for track in &scene.tracks {
            let kind = if track.kind == eb::TrackKind::Video {
                fp::TrackKind::Video
            } else {
                fp::TrackKind::Audio
            };
            let mut t = fp::Track::new(fp::TrackId(out.alloc_id()), kind, track.name.clone());
            t.enabled = track.enabled;
            for clip in &track.clips {
                let (item, base, origin, audio_stream) = match clip.source {
                    eb::ClipSource::Media { source, stream } => {
                        let source = project
                            .sources
                            .iter()
                            .find(|s| s.id == source)
                            .ok_or("Source absent")?;
                        let s = source
                            .streams
                            .iter()
                            .find(|s| s.index == stream)
                            .ok_or("Stream absent")?;
                        let audio = source
                            .streams
                            .iter()
                            .filter(|s| matches!(s.format, eb::StreamFormat::Audio { .. }))
                            .position(|s| s.index == stream)
                            .unwrap_or(0);
                        (ids[&source.id], s.time_base, s.start_tick, audio)
                    }
                    eb::ClipSource::Composition { composition } => {
                        let c = project
                            .compositions
                            .iter()
                            .find(|c| c.id == composition)
                            .ok_or("Nested composition absent")?;
                        (
                            ids[&composition],
                            eb::TimeBase {
                                numerator: c.frame_rate.denominator,
                                denominator: c.frame_rate.numerator,
                            },
                            0,
                            0,
                        )
                    }
                };
                let mut range = clip.range;
                for tr in project
                    .transitions
                    .iter()
                    .filter(|t| t.composition == scene.id)
                {
                    if tr.to == Some(clip.id) {
                        range.start = tr.cut;
                    }
                    if tr.from == Some(clip.id) {
                        range.end = tr.cut;
                    }
                }
                if range.end <= range.start {
                    return Err("Transition consumes the complete clip".into());
                }
                let a = clip.time_map.position(range.start - clip.range.start)?;
                let b = clip.time_map.position(range.end - clip.range.start)?;
                let a = tick(a.relative_to(origin)?, base)?;
                let b = tick(b.relative_to(origin)?, base)?;
                let start = frame_tick(range.start, scene.frame_rate)?;
                let duration = frame_tick(range.end - range.start, scene.frame_rate)?;
                let source_in = a.min(b);
                let source_duration = (b - a).abs();
                let mut c = out
                    .make_track_item(
                        item,
                        kind,
                        start,
                        filmcraft_time::TimeRange::new(source_in, source_duration.max(Tick(1))),
                        rate_out(scene.frame_rate),
                    )
                    .ok_or("Cannot create interchange clip")?;
                c.name = clip.name.clone();
                c.duration = duration;
                c.source_in = source_in;
                c.speed = source_duration.0 as f64 / duration.0 as f64;
                c.reverse = b < a;
                c.audio_stream = audio_stream;
                if a == b {
                    c.frame_hold = Some(a);
                    c.speed = 1.;
                }
                if clip.time_map.points.len() > 2 {
                    report.warn(format!(
                        "{} variable speed map reduced to endpoint speed",
                        clip.name
                    ));
                }
                if let Some(link) = clip.linked {
                    c.link = Some(u64::from_le_bytes(
                        clip.id.min(link).as_bytes()[..8].try_into()?,
                    ));
                }
                native_clips.insert(clip.id, c.id);
                t.items.push(c);
            }
            tracks.push(t);
        }
        for tr in project
            .transitions
            .iter()
            .filter(|t| t.composition == scene.id)
        {
            let track = tracks
                .iter_mut()
                .find(|t| {
                    t.items.iter().any(|c| {
                        Some(c.id) == tr.from.and_then(|id| native_clips.get(&id).copied())
                            || Some(c.id) == tr.to.and_then(|id| native_clips.get(&id).copied())
                    })
                })
                .ok_or("Transition track absent")?;
            let effect = fp::find_effect(if track.kind == fp::TrackKind::Video {
                "cross_dissolve"
            } else {
                "constant_gain"
            })
            .ok_or("Transition effect absent")?
            .instance();
            track.transitions.push(fp::Transition {
                id: fp::TransitionId(out.alloc_id()),
                effect,
                start: frame_tick(tr.range.start, scene.frame_rate)?,
                duration: frame_tick(tr.range.end - tr.range.start, scene.frame_rate)?,
                from: tr.from.and_then(|id| native_clips.get(&id).copied()),
                to: tr.to.and_then(|id| native_clips.get(&id).copied()),
                align: fp::TransitionAlign::CenterAtCut,
                reverse: false,
            });
        }
        let external = out.sequence_mut(ids[&scene.id]).ok_or("Sequence absent")?;
        for t in tracks {
            if t.kind == fp::TrackKind::Video {
                external.video_tracks.push(t);
            } else {
                external.audio_tracks.push(t);
            }
        }
        if scene.nodes.iter().any(|n| {
            !matches!(
                n.operation,
                eb::NodeOperation::Source { .. }
                    | eb::NodeOperation::Over { .. }
                    | eb::NodeOperation::Mix { .. }
                    | eb::NodeOperation::Solid { .. }
                    | eb::NodeOperation::Transform { .. }
                    | eb::NodeOperation::Gain { .. }
            )
        }) {
            report.warn(format!(
                "{} graph effects/title are not mapped to editorial effects",
                scene.name
            ));
        }
        if scene.nodes.iter().any(|n|matches!(n.operation,eb::NodeOperation::Transform{translation,scale,rotation,opacity,..}if translation!=[0.;2]||scale!=[1.;2]||rotation!=0.||opacity!=1.)||matches!(n.operation,eb::NodeOperation::Gain{gain,..}if gain!=1.)){report.warn(format!("{} placement/mix controls are not mapped",scene.name));}
    }
    Ok((out, ids, report))
}

fn rate_out(rate: eb::FrameRate) -> filmcraft_time::FrameRate {
    filmcraft_time::FrameRate::new(i64::from(rate.numerator), i64::from(rate.denominator))
}
fn frames(t: Tick, rate: eb::FrameRate) -> Result<u64> {
    let n = i128::from(t.0) * i128::from(rate.numerator);
    let d = i128::from(TICKS_PER_SECOND) * i128::from(rate.denominator);
    if n % d != 0 {
        return Err(
            "Editorial record boundary is not on the declared frame grid; conform explicitly"
                .into(),
        );
    }
    Ok(u64::try_from(n / d)?)
}
fn frame_tick(frame: u64, rate: eb::FrameRate) -> Result<Tick> {
    tick(
        eb::SourcePosition::new(frame.try_into()?, 1)?,
        eb::TimeBase {
            numerator: rate.denominator,
            denominator: rate.numerator,
        },
    )
}
fn tick(p: eb::SourcePosition, base: eb::TimeBase) -> Result<Tick> {
    let n = i128::from(p.numerator) * i128::from(base.numerator) * i128::from(TICKS_PER_SECOND);
    let d = i128::from(p.denominator) * i128::from(base.denominator);
    if n % d != 0 {
        return Err("Exact source time is not representable in the interchange tick clock".into());
    }
    Ok(Tick(i64::try_from(n / d)?))
}
fn fraction(n: i128, d: u64) -> Result<eb::SourcePosition> {
    let g = gcd(n.unsigned_abs(), u128::from(d));
    Ok(eb::SourcePosition::new(
        i64::try_from(n / i128::try_from(g)?)?,
        d / u64::try_from(g)?,
    )?)
}
fn add(a: eb::SourcePosition, b: eb::SourcePosition) -> Result<eb::SourcePosition> {
    let g = gcd(a.denominator.into(), b.denominator.into()) as u64;
    let d = (a.denominator / g)
        .checked_mul(b.denominator)
        .ok_or("Rational time overflow")?;
    fraction(
        i128::from(a.numerator) * i128::from(d / a.denominator)
            + i128::from(b.numerator) * i128::from(d / b.denominator),
        d,
    )
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}
fn time_map(duration: u64, a: eb::SourcePosition, b: eb::SourcePosition) -> Result<eb::TimeMap> {
    let d = (a.denominator / (gcd(a.denominator.into(), b.denominator.into()) as u64))
        .checked_mul(b.denominator)
        .ok_or("Time-map denominator overflow")?;
    Ok(eb::TimeMap {
        source_denominator: d.try_into()?,
        points: vec![
            eb::TimePoint {
                frame: 0,
                source_tick: i64::try_from(
                    i128::from(a.numerator) * i128::from(d / a.denominator),
                )?,
            },
            eb::TimePoint {
                frame: duration,
                source_tick: i64::try_from(
                    i128::from(b.numerator) * i128::from(d / b.denominator),
                )?,
            },
        ],
    })
}
fn decimal(v: f64) -> Result<(i64, u64)> {
    if !v.is_finite() || v.abs() > 1000. {
        return Err("Unsupported editorial speed".into());
    }
    let s = v.to_string();
    let (m, e) = s.split_once(['e', 'E']).map_or((s.as_str(), 0), |(m, e)| {
        (m, e.parse::<i32>().unwrap_or(100))
    });
    let (whole, frac) = m.split_once('.').unwrap_or((m, ""));
    let digits = format!("{whole}{frac}").parse::<i64>()?;
    let power = frac.len() as i32 - e;
    if power >= 0 {
        Ok((
            digits,
            10u64
                .checked_pow(power as u32)
                .ok_or("Speed precision overflow")?,
        ))
    } else {
        Ok((
            digits
                .checked_mul(10i64.checked_pow((-power) as u32).ok_or("Speed overflow")?)
                .ok_or("Speed overflow")?,
            1,
        ))
    }
}
