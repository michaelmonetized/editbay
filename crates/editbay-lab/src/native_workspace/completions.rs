use super::*;
use std::collections::HashSet;

pub(super) fn audit(records: &[Value], duration: u64) -> Result<Value> {
    let Some(summary) = records.iter().rev().find_map(|r| {
        (r["kind"] == "preview" && r["details"]["display"].is_object())
            .then_some(&r["details"]["display"])
    }) else {
        return Ok(json!({"supported":false,"complete_evidence":false}));
    };
    let session = &summary["session"];
    let scope = &summary["scope"];
    let ended = summary["observed_sound_end_us"]
        .as_u64()
        .ok_or("Display run has no observed sound end")?;
    if !session.is_string()
        || !scope.is_object()
        || summary["cancelled"] != false
        || summary["overflow"] != 0
        || summary["rejected"] != 0
        || summary["outside_history"] != 0
        || summary["first_frame"] != 0
        || summary["end_frame_exclusive"] != duration
    {
        return Err("Display session has incomplete or foreign evidence".into());
    }
    let mut unique = HashSet::new();
    let mut timely = HashSet::new();
    let mut events = Vec::new();
    for record in records
        .iter()
        .filter(|r| r["kind"] == "display" && &r["details"]["session"] == session)
    {
        let event = &record["details"];
        let frame = event["frame"].as_u64().ok_or("Display frame absent")?;
        if record["dropped_before"] != 0
            || &event["scope"] != scope
            || event["index"] != events.len() as u64 + 1
            || frame >= duration
        {
            return Err("Display event was lost, reordered or belongs to another document".into());
        }
        unique.insert(frame);
        if event["elapsed_us"]
            .as_u64()
            .ok_or("Display completion time absent")?
            <= ended
        {
            timely.insert(frame);
        }
        events.push(event.clone());
    }
    let missing = duration - timely.len() as u64;
    if summary["received"] != events.len() as u64
        || summary["unique_completed"] != unique.len() as u64
        || summary["completed_by_observed_end"] != timely.len() as u64
        || summary["missing_by_observed_end"] != missing
    {
        return Err(
            "Display completion events disagree with the bounded application counters".into(),
        );
    }
    let mut selection_ms: Vec<_> = events
        .iter()
        .filter_map(|e| e["selection_to_draw_us"].as_u64())
        .map(|us| us as f64 / 1000.)
        .collect();
    let prepared: Vec<_> = records
        .iter()
        .filter(|r| {
            r["kind"] == "preview"
                && &r["details"]["display"]["session"] == session
                && r["details"]["prepared_pictures"].is_object()
        })
        .collect();
    let mut maximum_queued = 0;
    let mut maximum_prepared = 0;
    for r in &prepared {
        let q = &r["details"]["prepared_pictures"];
        let capacity = q["capacity"]
            .as_u64()
            .ok_or("Prepared picture capacity absent")?;
        let queued = q["queued"]
            .as_u64()
            .ok_or("Prepared picture occupancy absent")?;
        let pending = u64::from(
            q["in_flight"]
                .as_bool()
                .ok_or("Prepared picture work state absent")?,
        );
        let bytes = q["maximum_picture_bytes"]
            .as_u64()
            .ok_or("Prepared picture size absent")?;
        if !(1..=8).contains(&capacity)
            || queued + pending > capacity
            || q["first"] != 0
            || q["end"] != duration
            || q["maximum_pinned_bytes"] != bytes * (capacity + 1)
            || bytes * (capacity + 1) > 256 * 1024 * 1024
            || q["session"] != prepared[0]["details"]["prepared_pictures"]["session"]
        {
            return Err("Prepared pictures exceeded their declared bounds or changed owner".into());
        }
        maximum_queued = maximum_queued.max(queued);
        maximum_prepared = maximum_prepared.max(
            q["prepared"]
                .as_u64()
                .ok_or("Prepared picture count absent")?,
        );
    }
    let first_sound = prepared
        .iter()
        .find(|r| r["details"]["sound_active"] == true);
    if !prepared.is_empty()
        && first_sound.is_none_or(|r| {
            r["details"]["displayed_frame"] != 0
                || r["details"]["gpu_draw_completed_us"]
                    .as_u64()
                    .is_none_or(|us| us == 0)
        })
    {
        return Err("Sound started before the first prepared picture completed its draw".into());
    }
    Ok(
        json!({"supported":true,"complete_evidence":true,"summary":summary,
        "missing_by_observed_end":missing,"every_frame_completed_by_observed_end":missing == 0,"events":events,
        "selection_to_gpu_completion":(!selection_ms.is_empty()).then(||metrics(&mut selection_ms)),
        "prepared_queue": (!prepared.is_empty()).then(||json!({"bounded":true,"maximum_queued":maximum_queued,
            "prepared":maximum_prepared,"first_picture_completed_before_sound_started":true}))}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> Vec<Value> {
        let scope = json!({"tab":"tab","version":{"revision":2},"sequence":"sequence"});
        let mut records = Vec::new();
        for (index, frame, elapsed) in [(1, 1, 10), (2, 3, 20), (3, 2, 15), (4, 4, 30)] {
            records.push(json!({"kind":"display","dropped_before":0,"details":{
                "session":"run","scope":scope,"index":index,"frame":frame,"elapsed_us":elapsed}}));
        }
        records.push(json!({"kind":"preview","details":{"display":{
            "session":"run","scope":scope,"first_frame":0,"end_frame_exclusive":5,
            "observed_sound_end_us":25,"cancelled":false,"overflow":0,"rejected":0,"outside_history":0,
            "received":4,"unique_completed":4,"completed_by_observed_end":3,"missing_by_observed_end":2}}}));
        records
    }

    #[test]
    fn actual_events_keep_initial_tail_and_late_omissions() {
        let receipt = audit(&records(), 5).unwrap();
        assert_eq!(receipt["missing_by_observed_end"], 2);
        assert_eq!(receipt["complete_evidence"], true);
        assert_eq!(receipt["every_frame_completed_by_observed_end"], false);
        assert_eq!(audit(&[], 5).unwrap()["supported"], false);
    }

    #[test]
    fn missing_foreign_or_overflowed_events_cannot_pass() {
        for mutation in 0..5 {
            let mut records = records();
            match mutation {
                0 => {
                    records.remove(0);
                }
                1 => records[0]["details"]["scope"]["tab"] = json!("other"),
                2 => records[0]["dropped_before"] = json!(1),
                3 => records[4]["details"]["display"]["overflow"] = json!(1),
                _ => records[4]["details"]["display"]["missing_by_observed_end"] = json!(0),
            }
            assert!(audit(&records, 5).is_err());
        }
    }

    #[test]
    fn prepared_bounds_and_first_draw_must_precede_sound() {
        let mut records = records();
        let d = &mut records[4]["details"];
        d["prepared_pictures"] = json!({"session":"queue","first":0,"end":5,"capacity":4,
            "queued":3,"in_flight":true,"maximum_picture_bytes":16,"maximum_pinned_bytes":80,"prepared":4});
        d["sound_active"] = json!(true);
        d["displayed_frame"] = json!(0);
        d["gpu_draw_completed_us"] = json!(10);
        assert_eq!(
            audit(&records, 5).unwrap()["prepared_queue"]["bounded"],
            true
        );
        for field in ["queued", "maximum_pinned_bytes", "end"] {
            let mut altered = records.clone();
            altered[4]["details"]["prepared_pictures"][field] = json!(100);
            assert!(audit(&altered, 5).is_err());
        }
        records[4]["details"]["gpu_draw_completed_us"] = json!(0);
        assert!(audit(&records, 5).is_err());
    }
}
