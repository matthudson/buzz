use super::*;

#[test]
fn runtime_tree_digest_uses_global_relative_path_order() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("a")).unwrap();
    std::fs::write(directory.path().join("a/file"), b"nested").unwrap();
    std::fs::write(directory.path().join("a.txt"), b"sibling").unwrap();

    let mut expected = Sha256::new();
    for (relative, bytes) in [("a.txt", b"sibling".as_slice()), ("a/file", b"nested")] {
        expected.update(relative.as_bytes());
        expected.update([0]);
        expected.update(bytes.len().to_string().as_bytes());
        expected.update([0]);
        expected.update(bytes);
    }
    assert_eq!(
        runtime_tree_digest(directory.path()).unwrap(),
        hex::encode(expected.finalize())
    );
}

#[test]
fn event_page_must_end_at_the_exact_projected_checkpoint() {
    let checkpoint = LastVerifiedUpdate {
        sequence: 2,
        revision: 2,
        kind: "implementing".into(),
        observed_at: "2026-09-15T12:00:00.000Z".into(),
    };
    let valid = vec![
        ControllerEvent {
            sequence: 1,
            revision: 1,
            kind: "created".into(),
            observed_at: "2026-09-15T11:59:00.000Z".into(),
        },
        ControllerEvent {
            sequence: 2,
            revision: 2,
            kind: "implementing".into(),
            observed_at: checkpoint.observed_at.clone(),
        },
    ];
    assert!(validate_event_snapshot(&valid, &checkpoint).is_ok());

    let mut newer = valid.clone();
    newer.push(ControllerEvent {
        sequence: 3,
        revision: 3,
        kind: "reviewing".into(),
        observed_at: "2026-09-15T12:01:00.000Z".into(),
    });
    assert!(validate_event_snapshot(&newer, &checkpoint).is_err());

    let duplicated = vec![valid[1].clone(), valid[1].clone()];
    assert!(validate_event_snapshot(&duplicated, &checkpoint).is_err());
}
