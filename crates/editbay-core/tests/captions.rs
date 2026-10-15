use editbay_core::{FrameRate, captions::*};
use uuid::Uuid;

fn composition() -> Uuid {
    Uuid::from_u128(10)
}

fn ms(value: u64) -> CaptionTime {
    CaptionTime::from_milliseconds(value)
}

fn text_and_times(track: &CaptionTrack) -> Vec<(CaptionTime, CaptionTime, &str)> {
    track
        .cues
        .iter()
        .map(|cue| (cue.start, cue.end, cue.text.as_str()))
        .collect()
}

#[test]
fn srt_crlf_bom_unicode_and_overlapping_cues_round_trip_through_vtt() {
    let input = "\u{feff}9\r\n00:00:00,001 --> 00:00:02,003\r\nCafé 🎬\r\nمرحبا 世界\r\n \t\r\n42\r\n00:00:01,007 --> 123:45:56,789\r\nSecond line\r\n\r\n";
    let imported = CaptionTrack::import_srt(composition(), "Interview", input).unwrap();
    assert_eq!(imported.cues.len(), 2);
    assert_eq!(imported.cues[0].text, "Café 🎬\nمرحبا 世界");
    assert_eq!(imported.cues[0].start, ms(1));
    assert_eq!(imported.cues[0].end, ms(2003));
    assert_eq!(imported.cues[1].end, ms(445_556_789));
    let vtt = imported.export_vtt().unwrap();
    assert!(vtt.starts_with("WEBVTT\n\n9\n00:00:00.001 --> 00:00:02.003"));
    let from_vtt = CaptionTrack::import_vtt(composition(), "VTT", &vtt).unwrap();
    assert_eq!(text_and_times(&imported), text_and_times(&from_vtt));
    let srt = from_vtt.export_srt().unwrap();
    assert!(srt.starts_with("1\n00:00:00,001 --> 00:00:02,003"));
    let from_srt = CaptionTrack::import_srt(composition(), "Reopened", &srt).unwrap();
    assert_eq!(text_and_times(&imported), text_and_times(&from_srt));
}

#[test]
fn vtt_preserves_header_identifiers_settings_styles_regions_and_comments() {
    let input = "WEBVTT Caption fixture\nKind: captions\nLanguage: en\n\nNOTE preserved comment\nCafé\n\nSTYLE\n::cue { color: lime; }\n\nREGION\nid:main\nwidth:80%\n\nopening\n00:01.001 --> 00:02.002 line:90%,end position:50%,center size:80% align:center region:main\n<v Zoë><c.green>Hello 世界</c></v>\n\nNOTE a later comment\n\n00:02.002 --> 00:04.003 vertical:rl\nNext\n";
    let track = CaptionTrack::import_vtt(composition(), "Captions", input).unwrap();
    assert_eq!(track.delivery, CaptionDelivery::SidecarVtt);
    assert_eq!(track.vtt_header, "Caption fixture");
    assert_eq!(track.vtt_metadata, ["Kind: captions", "Language: en"]);
    assert_eq!(track.vtt_blocks.len(), 4);
    assert_eq!(track.cues[0].identifier.as_deref(), Some("opening"));
    assert_eq!(track.cues[0].start, ms(1001));
    let output = track.export_vtt().unwrap();
    let reopened = CaptionTrack::import_vtt(composition(), "Reopened", &output).unwrap();
    assert_eq!(text_and_times(&track), text_and_times(&reopened));
    assert_eq!(track.vtt_header, reopened.vtt_header);
    assert_eq!(track.vtt_metadata, reopened.vtt_metadata);
    assert_eq!(track.vtt_blocks, reopened.vtt_blocks);
    assert_eq!(track.cues[0].vtt_settings, reopened.cues[0].vtt_settings);
    assert!(track.export_srt().is_err());
    assert!(track.validate_burn_in().is_err());
}

#[test]
fn exact_frame_times_never_round_and_half_open_cues_select_correctly() {
    let rate = FrameRate::new(30_000, 1001).unwrap();
    let start = CaptionTime::from_frame(1, rate).unwrap();
    let end = CaptionTime::from_frame(2, rate).unwrap();
    assert_eq!(start, CaptionTime::new(1001, 30_000).unwrap());
    assert!(start.to_milliseconds().is_err());
    assert_eq!(CaptionTime::from_frame(30, rate).unwrap(), ms(1001));
    let mut track = CaptionTrack::new(composition(), "Exact").unwrap();
    track
        .insert_cue(CaptionCue::new(start, end, "Frame one").unwrap())
        .unwrap();
    assert!(track.active_at_frame(0, rate).unwrap().is_empty());
    assert_eq!(track.active_at_frame(1, rate).unwrap().len(), 1);
    assert!(track.active_at_frame(2, rate).unwrap().is_empty());
    assert!(track.export_srt().is_err());
    assert!(track.export_vtt().is_err());
    track.enabled = false;
    assert!(track.active_at(start).unwrap().is_empty());
}

#[test]
fn cue_edit_split_retime_shift_and_serialization_preserve_unicode_and_identity() {
    let mut track = CaptionTrack::new(composition(), "Transcript").unwrap();
    let cue = CaptionCue::new(ms(1000), ms(4000), "Café 🎬 世界").unwrap();
    let id = cue.id;
    track.insert_cue(cue).unwrap();
    let second = track.split_cue(id, ms(2500), 6).unwrap();
    assert_ne!(id, second);
    assert_eq!(track.cues[0].id, id);
    assert_eq!(track.cues[0].text, "Café 🎬");
    assert_eq!(track.cues[1].text, "世界");
    assert_eq!(track.cues[0].end, track.cues[1].start);
    track.edit_cue(second, "مرحبا 世界").unwrap();
    track.retime_cue(second, ms(500), ms(900)).unwrap();
    assert_eq!(track.cues[0].id, second);
    track.shift_milliseconds(125).unwrap();
    assert_eq!(track.cues[0].start, ms(625));
    assert_eq!(track.cues[0].end, ms(1025));
    let json = serde_json::to_string(&track).unwrap();
    let reopened: CaptionTrack = serde_json::from_str(&json).unwrap();
    reopened.validate().unwrap();
    assert_eq!(reopened, track);
    let output = reopened.export_srt().unwrap();
    let parsed = CaptionTrack::import_srt(composition(), "Independent import", &output).unwrap();
    assert_eq!(text_and_times(&reopened), text_and_times(&parsed));
}

#[test]
fn failed_edits_are_atomic_and_missing_or_duplicate_ids_are_rejected() {
    let mut track = CaptionTrack::new(composition(), "Atomic").unwrap();
    let cue = CaptionCue::new(ms(100), ms(500), "Hello 世界").unwrap();
    let id = cue.id;
    track.insert_cue(cue.clone()).unwrap();
    let original = track.clone();
    assert!(track.insert_cue(cue).is_err());
    assert!(track.edit_cue(id, "\n").is_err());
    assert!(track.edit_cue(Uuid::nil(), "Missing").is_err());
    assert!(track.retime_cue(id, ms(500), ms(500)).is_err());
    assert!(track.retime_cue(Uuid::nil(), ms(1), ms(2)).is_err());
    assert!(track.split_cue(id, ms(100), 5).is_err());
    assert!(track.split_cue(id, ms(500), 5).is_err());
    assert!(track.split_cue(id, ms(200), 0).is_err());
    assert!(track.split_cue(id, ms(200), 100).is_err());
    assert!(track.shift_milliseconds(-101).is_err());
    assert_eq!(track, original);
}

#[test]
fn negative_shifts_keep_submillisecond_fractions_exact_and_reject_underflow() {
    let time = CaptionTime::new(1001, 30_000).unwrap();
    assert_eq!(
        time.shifted_milliseconds(1).unwrap(),
        CaptionTime::new(1031, 30_000).unwrap()
    );
    assert_eq!(
        time.shifted_milliseconds(-1).unwrap(),
        CaptionTime::new(971, 30_000).unwrap()
    );
    assert!(time.shifted_milliseconds(-34).is_err());
    assert_eq!(ms(1000).shifted_milliseconds(-1000).unwrap(), ms(0));
    assert!(CaptionTime::new(1, 0).is_err());
    assert!(
        CaptionTime {
            numerator: 2,
            denominator: 4
        }
        .validate()
        .is_err()
    );
    assert!(
        CaptionTime {
            numerator: 0,
            denominator: 0
        }
        .to_milliseconds()
        .is_err()
    );
    assert!(
        CaptionTime::new(u64::MAX, 1)
            .unwrap()
            .to_milliseconds()
            .is_err()
    );
    assert!(CaptionTime::from_frame(u64::MAX, FrameRate::new(1, u32::MAX).unwrap()).is_err());
    assert_eq!(
        CaptionTime::from_milliseconds(u64::MAX)
            .to_milliseconds()
            .unwrap(),
        u64::MAX
    );
}

#[test]
fn safe_area_style_and_delivery_choices_have_validated_durable_defaults() {
    let mut style = CaptionStyle::default();
    assert_eq!(
        style.safe_area.bounds(1920, 1080).unwrap(),
        [192.0, 108.0, 1728.0, 972.0]
    );
    assert_eq!(serde_json::from_str::<CaptionStyle>("{}").unwrap(), style);
    style.font = Some(Uuid::from_u128(99));
    style.validate().unwrap();
    style.safe_area.left = 0.95;
    assert!(style.validate().is_err());
    style = CaptionStyle::default();
    style.foreground[0] = f32::NAN;
    assert!(style.validate().is_err());
    style = CaptionStyle::default();
    style.font = Some(Uuid::nil());
    assert!(style.validate().is_err());
    assert!(CaptionSafeArea::default().bounds(0, 1080).is_err());
    for (choice, burns, extension) in [
        (CaptionDelivery::None, false, None),
        (CaptionDelivery::SidecarSrt, false, Some("srt")),
        (CaptionDelivery::SidecarVtt, false, Some("vtt")),
        (CaptionDelivery::BurnIn, true, None),
        (CaptionDelivery::BurnInAndSrt, true, Some("srt")),
        (CaptionDelivery::BurnInAndVtt, true, Some("vtt")),
    ] {
        assert_eq!(choice.burns_in(), burns);
        assert_eq!(choice.sidecar_extension(), extension);
    }
}

#[test]
fn malformed_srt_and_vtt_files_fail_without_guessing_timing() {
    for input in [
        "1\n00:00:00,00 --> 00:00:02,000\nBad precision\n",
        "1\n00:00:00.000 --> 00:00:02.000\nWrong separator\n",
        "1\n00:60:00,000 --> 01:00:02,000\nBad minute\n",
        "1\n00:00:60,000 --> 00:01:02,000\nBad second\n",
        "1\n00:00:02,000 --> 00:00:01,000\nReversed\n",
        "1\n00:00:01,000 --> 00:00:01,000\nEmpty range\n",
        "1\n00:00:01,000 --> 00:00:02,000\n\n",
        "name\n00:00:01,000 --> 00:00:02,000\nBad index\n",
        "1\n00:00:01,000 --> 00:00:02,000 --> 00:00:03,000\nArrows\n",
        "1\n99999999999999999999999:00:00,000 --> 99999999999999999999999:00:01,000\nOverflow\n",
        "1\n00:00:00,000 --> 00:00:02,000\nNUL\0\n",
        "1\n00:00:00,000 --> 00:00:02,000 X1:0 X2:10\nUnsupported positioning\n",
    ] {
        assert!(
            CaptionTrack::import_srt(composition(), "Invalid", input).is_err(),
            "accepted SRT: {input:?}"
        );
    }
    for input in [
        "00:00.000 --> 00:01.000\nNo header\n",
        "WEBVTT\n00:00.000 --> 00:01.000\nNo blank line\n",
        "WEBVTT\n\n00:00,000 --> 00:01,000\nWrong separator\n",
        "WEBVTT\n\n00:00.000 --> 00:01.000 size:101%\nBad percentage\n",
        "WEBVTT\n\n00:00.000 --> 00:01.000 align:banana\nBad alignment\n",
        "WEBVTT\n\n00:00.000 --> 00:01.000 align:left align:right\nDuplicate\n",
        "WEBVTT\n\n00:00.000 --> 00:01.000 unsupported:true\nUnknown\n",
        "WEBVTT\n\n00:00.000 --> 00:01.000\nA --> B\n",
        "WEBVTT\n\n00:00.000 --> 00:01.000\nFirst\n\nSTYLE\n::cue { color:red }\n",
        "WEBVTT\n\n00:02.000 --> 00:03.000\nLater\n\n00:01.000 --> 00:02.000\nEarlier\n",
    ] {
        assert!(
            CaptionTrack::import_vtt(composition(), "Invalid", input).is_err(),
            "accepted VTT: {input:?}"
        );
    }
}

#[test]
fn plain_unicode_burn_in_and_cross_format_markup_have_explicit_limits() {
    let mut track = CaptionTrack::import_srt(
        composition(),
        "Plain",
        "1\n00:00:00,000 --> 00:00:01,000\nCafé 🎬 & tea\n",
    )
    .unwrap();
    track.validate_burn_in().unwrap();
    let id = track.cues[0].id;
    track.edit_cue(id, "<b>Bold</b>").unwrap();
    assert!(track.validate_burn_in().is_err());
    assert!(track.export_srt().is_ok());
    assert!(track.export_vtt().is_ok());
    track.edit_cue(id, "<v Zoë>Voice</v>").unwrap();
    assert!(track.export_srt().is_err());
    assert!(track.export_vtt().is_ok());
    track
        .edit_cue(id, "<font color=\"red\">Color</font>")
        .unwrap();
    assert!(track.export_srt().is_ok());
    assert!(track.export_vtt().is_err());
}

#[test]
fn empty_tracks_and_optional_srt_indices_work_but_duplicate_and_unordered_cues_fail() {
    assert!(
        CaptionTrack::import_srt(composition(), "Empty", "")
            .unwrap()
            .cues
            .is_empty()
    );
    let empty = CaptionTrack::import_vtt(composition(), "Empty", "WEBVTT\n").unwrap();
    assert_eq!(empty.export_vtt().unwrap(), "WEBVTT\n\n");
    let mut track = CaptionTrack::import_srt(
        composition(),
        "No index",
        "00:00:01,000 --> 00:00:02,000\nHello\n",
    )
    .unwrap();
    assert_eq!(track.cues.len(), 1);
    let mut second = track.cues[0].clone();
    second.start = ms(0);
    second.end = ms(500);
    track.cues.push(second);
    assert!(track.validate().is_err());
    track.cues[1].id = Uuid::new_v4();
    assert!(track.validate().is_err());
    track.cues.swap(0, 1);
    track.validate().unwrap();
}
