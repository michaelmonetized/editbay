use editbay_core::{checkpoint, load, recover_copy, save_new};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[test]
fn immutable_reference_replays_native_roundtrip_and_recovery_without_conversion_loss() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/r0-reference");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("fixtures.json")).unwrap()).unwrap();
    for fixture in manifest["fixtures"].as_array().unwrap() {
        let bytes = fs::read(root.join(fixture["path"].as_str().unwrap())).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            fixture["sha256"].as_str().unwrap()
        );
    }
    let original_path = root.join("fixtures/foundation.editbay");
    let original_bytes = fs::read(&original_path).unwrap();
    let original = load(&original_path).unwrap();
    let expected = &manifest["fixtures"][0]["expected"];
    assert_eq!(original.id.to_string(), expected["project_id"]);
    assert_eq!(original.revision, expected["revision"].as_u64().unwrap());
    assert_eq!(
        original.sequences.len(),
        expected["sequence_count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        original.sequences[2]
            .frame_rate
            .frame_nanoseconds(120)
            .unwrap(),
        u128::from(
            expected["fractional_frame_120_nanoseconds"]
                .as_u64()
                .unwrap()
        )
    );
    let directory = tempfile::tempdir().unwrap();
    let roundtrip = directory.path().join("Round trip.editbay");
    save_new(&original, &roundtrip).unwrap();
    assert_eq!(load(&roundtrip).unwrap(), original);
    let recovery_root = directory.path().join("Recovery");
    let snapshot = checkpoint(&original, Some(&original_path), &recovery_root).unwrap();
    let checkpoint_bytes = fs::read(&snapshot).unwrap();
    assert!(recover_copy(&snapshot, &original_path).is_err());
    let recovered_path = directory.path().join("Recovered.editbay");
    let recovered = recover_copy(&snapshot, &recovered_path).unwrap();
    assert_ne!(recovered.id, original.id);
    assert_eq!(recovered.recovered_from, Some(original.id));
    assert_eq!(recovered.revision, original.revision);
    assert_eq!(recovered.name, original.name);
    assert_eq!(recovered.sequences, original.sequences);
    assert_eq!(load(&recovered_path).unwrap(), recovered);
    assert!(recover_copy(&snapshot, &recovered_path).is_err());
    assert_eq!(fs::read(&snapshot).unwrap(), checkpoint_bytes);
    assert_eq!(fs::read(&original_path).unwrap(), original_bytes);
    assert!(load(root.join("fixtures/migration-with-losses.fcpxml")).is_err());
}
