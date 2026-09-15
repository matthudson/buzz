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

#[test]
fn runtime_traversal_bounds_entries_depth_and_bounded_file_reads() {
    let mut entries = MAX_RUNTIME_ENTRIES;
    assert!(count_runtime_entry(&mut entries).is_err());

    let root = tempfile::tempdir().unwrap();
    let mut nested = root.path().to_path_buf();
    for _ in 0..=MAX_RUNTIME_DEPTH {
        nested.push("d");
        std::fs::create_dir(&nested).unwrap();
    }
    assert!(runtime_tree_digest(root.path()).is_err());

    let bounded = root.path().join("bounded.json");
    std::fs::write(&bounded, b"123456789").unwrap();
    assert!(read_bounded_file(&bounded, 8, "bounded").is_err());
}

#[test]
fn production_traversal_applies_the_total_entry_limit() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("first")).unwrap();
    std::fs::create_dir(root.path().join("second")).unwrap();
    let mut files = Vec::new();
    let mut entries_seen = MAX_RUNTIME_ENTRIES - 1;
    assert!(
        collect_runtime_files(root.path(), root.path(), &mut files, &mut entries_seen, 0).is_err()
    );
}

#[test]
fn workspace_validation_rejects_an_oversized_envelope_through_the_real_path() {
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    let repository = root.path().join("repository");
    let state = root.path().join("state");
    std::fs::create_dir_all(runtime.join("scripts/orchestrator")).unwrap();
    std::fs::create_dir(&repository).unwrap();
    std::fs::create_dir(&state).unwrap();
    let script = runtime.join("scripts/orchestrator/development-mcp.mjs");
    let node = root.path().join("node.exe");
    let codex = root.path().join("codex.exe");
    let devin = root.path().join("devin.exe");
    let envelope = root.path().join("envelope.json");
    for path in [&script, &node, &codex, &devin] {
        std::fs::write(path, b"fixture").unwrap();
    }
    std::fs::write(&envelope, vec![b' '; MAX_ENVELOPE_BYTES as usize + 1]).unwrap();
    let config = WorkspaceConfig {
        relay_origin: "https://relay.example".into(),
        channel_id: "project-channel".into(),
        repo_address: "30617:owner:repo".into(),
        project_id: "project".into(),
        principal_id: "buzz-desktop".into(),
        node_executable: node.to_string_lossy().into_owned(),
        node_sha256: sha256_file(&node).unwrap(),
        runtime_root: runtime.to_string_lossy().into_owned(),
        runtime_revision: "a".repeat(40),
        runtime_script_sha256: sha256_file(&script).unwrap(),
        runtime_tree_sha256: runtime_tree_digest(&runtime).unwrap(),
        repository_path: repository.to_string_lossy().into_owned(),
        state_dir: state.to_string_lossy().into_owned(),
        envelope_path: envelope.to_string_lossy().into_owned(),
        envelope_digest: "b".repeat(64),
        codex_executable: codex.to_string_lossy().into_owned(),
        codex_sha256: sha256_file(&codex).unwrap(),
        devin_executable: devin.to_string_lossy().into_owned(),
        devin_sha256: sha256_file(&devin).unwrap(),
        default_checks: vec!["repository".into()],
        default_worker: "swe-2-direct".into(),
        default_reviewer: "codex-sol".into(),
        data_class: "INTERNAL".into(),
        resource_plan: None,
    };
    match validate_workspace(config) {
        Err(error) => assert_eq!(error, "vyzr_workspace_envelope_unavailable"),
        Ok(_) => panic!("oversized envelope unexpectedly passed validation"),
    }
}

#[test]
fn cached_client_path_reuses_only_the_previously_validated_config() {
    let cached = WorkspaceConfig {
        relay_origin: "https://relay.example".into(),
        channel_id: "project-channel".into(),
        repo_address: "30617:owner:repo".into(),
        project_id: "project".into(),
        principal_id: "buzz-desktop".into(),
        node_executable: "validated-node.exe".into(),
        node_sha256: "a".repeat(64),
        runtime_root: "validated-runtime".into(),
        runtime_revision: "b".repeat(40),
        runtime_script_sha256: "c".repeat(64),
        runtime_tree_sha256: "d".repeat(64),
        repository_path: "validated-repository".into(),
        state_dir: "validated-state".into(),
        envelope_path: "validated-envelope.json".into(),
        envelope_digest: "e".repeat(64),
        codex_executable: "validated-codex.exe".into(),
        codex_sha256: "f".repeat(64),
        devin_executable: "validated-devin.exe".into(),
        devin_sha256: "1".repeat(64),
        default_checks: vec!["repository".into()],
        default_worker: "swe-2-direct".into(),
        default_reviewer: "codex-sol".into(),
        data_class: "INTERNAL".into(),
        resource_plan: None,
    };
    assert_eq!(
        cached_operation_config("same", "same", &cached)
            .unwrap()
            .node_executable,
        "validated-node.exe"
    );
    match cached_operation_config("old", "new", &cached) {
        Err(error) => assert_eq!(
            error,
            "vyzr_workspace_configuration_changed_restart_required"
        ),
        Ok(_) => panic!("changed configuration unexpectedly reused a cached client"),
    }
}
