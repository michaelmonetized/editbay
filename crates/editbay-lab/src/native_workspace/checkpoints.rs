use editbay_core::DocumentVersion;
use serde_json::Value;

pub(super) fn acknowledged(record: &Value, after: u64, expected: &[DocumentVersion]) -> bool {
    record["kind"] == "workspace"
        && record["unix_us"].as_u64().is_some_and(|time| time >= after)
        && record["details"]["tabs"].as_array().is_some_and(|tabs| {
            !expected.is_empty()
                && tabs.len() == expected.len()
                && expected.iter().all(|version| {
                    tabs.iter().any(|tab| {
                        tab["project"] == version.project_id.to_string()
                            && tab["revision"] == version.revision
                            && tab["recovery_revision"] == version.revision
                    })
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn earlier_current_checkpoints_cannot_acknowledge_newer_requested_revisions() {
        let active = DocumentVersion {
            project_id: Uuid::new_v4(),
            revision: 3,
        };
        let inactive = DocumentVersion {
            project_id: Uuid::new_v4(),
            revision: 0,
        };
        let expected = [active, inactive];
        let mut record = json!({"kind":"workspace","unix_us":100,"details":{"tabs":[
            {"project":active.project_id,"revision":0,"recovery_revision":0},
            {"project":inactive.project_id,"revision":0,"recovery_revision":0}
        ]}});
        assert!(!acknowledged(&record, 100, &expected));
        record["details"]["tabs"][0]["revision"] = json!(3);
        assert!(!acknowledged(&record, 100, &expected));
        record["details"]["tabs"][0]["recovery_revision"] = json!(3);
        assert!(acknowledged(&record, 100, &expected));
        assert!(!acknowledged(&record, 101, &expected));
        record["details"]["tabs"][1]["project"] = json!(Uuid::new_v4());
        assert!(!acknowledged(&record, 100, &expected));
        assert!(!acknowledged(&record, 100, &[]));
    }
}
