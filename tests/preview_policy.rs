//! Domain-port tests, not claims of Git/filesystem/network adapter safety.
use bootleg_cli::preview::*;
use bootleg_core::{BuildCommit, ContainerId};

fn commit(c: char) -> BuildCommit {
    c.to_string().repeat(40).parse().unwrap()
}
fn target() -> Target {
    Target {
        container_id: ContainerId::new(),
        build_commit: Some(commit('a')),
        build_dirty: Some(false),
        connected: true,
    }
}
fn accepted(t: &Target) -> AcceptedTarget {
    select_targets(
        std::slice::from_ref(&t.container_id),
        std::slice::from_ref(t),
    )
    .unwrap()
    .remove(0)
}
fn side(bytes: &[u8]) -> Option<TextSide<'_>> {
    Some(TextSide {
        mode: Mode::Regular,
        bytes,
    })
}
fn codes(errors: Vec<Diagnostic>) -> Vec<String> {
    serde_json::to_value(errors)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn selection_is_explicit_sorted_deduplicated_and_all_or_nothing() {
    let a = target();
    let b = target();
    let selected = select_targets(
        &[
            b.container_id.clone(),
            a.container_id.clone(),
            b.container_id.clone(),
        ],
        &[a.clone(), b.clone(), a.clone()],
    )
    .unwrap();
    assert_eq!(selected.len(), 2);
    assert!(selected[0].container_id.to_string() < selected[1].container_id.to_string());
    assert_eq!(
        codes(select_targets(&[], &[]).unwrap_err()),
        ["limit_exceeded"]
    );
    let missing = ContainerId::new();
    assert_eq!(
        codes(select_targets(&[a.container_id.clone(), missing], &[a]).unwrap_err()),
        ["target_unavailable"]
    );
    let too_many: Vec<_> = (0..=MAX_TARGETS).map(|_| ContainerId::new()).collect();
    assert!(select_targets(&too_many, &[]).is_err());
}

#[test]
fn unknown_dirty_offline_and_inconsistent_duplicates_fail_closed() {
    for (dirty, known, connected, expected) in [
        (None, true, true, "baseline_unknown"),
        (Some(false), false, true, "baseline_unknown"),
        (Some(true), true, true, "baseline_dirty"),
        (Some(false), true, false, "target_unavailable"),
    ] {
        let mut t = target();
        t.build_dirty = dirty;
        t.connected = connected;
        if !known {
            t.build_commit = None;
        }
        assert_eq!(
            codes(select_targets(std::slice::from_ref(&t.container_id), &[t.clone()]).unwrap_err()),
            [expected]
        );
    }
    let t = target();
    let mut other = t.clone();
    other.build_commit = Some(commit('b'));
    assert_eq!(
        codes(
            select_targets(std::slice::from_ref(&t.container_id), &[t.clone(), other]).unwrap_err()
        ),
        ["discovery_failed"]
    );
}

#[test]
fn history_requires_objects_head_and_positive_proof() {
    let a = commit('a');
    let b = commit('b');
    assert_eq!(
        relationship(&a, Some(&a), true, None),
        Ok(Relationship::Equal)
    );
    assert_eq!(
        relationship(&a, Some(&b), true, Some(true)),
        Ok(Relationship::Ancestor)
    );
    for proof in [Some(false), None] {
        assert_eq!(
            relationship(&a, Some(&b), true, proof),
            Err(Code::UnsafeHistory)
        );
    }
    assert_eq!(
        relationship(&a, None, true, Some(true)),
        Err(Code::UnsupportedRepository)
    );
    assert_eq!(
        relationship(&a, Some(&a), false, Some(true)),
        Err(Code::MissingObject)
    );
    let sha256: BuildCommit = "a".repeat(64).parse().unwrap();
    assert_eq!(
        relationship(&sha256, Some(&a), true, Some(true)),
        Err(Code::MissingObject)
    );
}

#[test]
fn final_bytes_raw_newlines_add_delete_and_modes() {
    assert!(
        compare_text(b"a", side(b"same"), side(b"same"), true)
            .unwrap()
            .is_none()
    );
    assert!(compare_text(b"a", None, None, false).unwrap().is_none());
    for (old, new, kind) in [
        (None, side(b"text"), "add"),
        (side(b"text"), None, "delete"),
        (side(b"a\r\n"), side(b"a\n"), "modify"),
        (side(b"a"), side(b"a\n"), "modify"),
    ] {
        let value =
            serde_json::to_value(compare_text(b"a", old, new, false).unwrap().unwrap()).unwrap();
        assert_eq!(value["kind"], kind);
        assert_eq!(
            value["old_bytes"],
            serde_json::to_value(old.map(|s| s.bytes.len())).unwrap()
        );
        assert_eq!(
            value["new_bytes"],
            serde_json::to_value(new.map(|s| s.bytes.len())).unwrap()
        );
        assert_eq!(value.as_object().unwrap().len(), 6);
    }
    let executable = Some(TextSide {
        mode: Mode::Executable,
        bytes: b"text",
    });
    assert!(compare_text(b"a", None, executable, false).is_ok());
    assert!(compare_text(b"a", executable, None, false).is_ok());
    assert_eq!(
        compare_text(b"a", side(b"text"), executable, false),
        Err(Code::UnsupportedEntry)
    );
}

#[test]
fn binary_attributes_and_file_size_gates_only_changed_entries() {
    for binary in [b"a\0".as_slice(), b"\xff"] {
        assert_eq!(
            compare_text(b"a", None, side(binary), false),
            Err(Code::UnsupportedEntry)
        );
        assert_eq!(
            compare_text(b"a", side(binary), None, false),
            Err(Code::UnsupportedEntry)
        );
        assert!(
            compare_text(b"a", side(binary), side(binary), false)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        compare_text(b"a", None, side(b"pointer"), true),
        Err(Code::UnsupportedAttributes)
    );
    let at_limit = vec![b'x'; MAX_FILE_BYTES];
    assert!(compare_text(b"a", None, side(&at_limit), false).is_ok());
    let over_limit = vec![b'x'; MAX_FILE_BYTES + 1];
    assert_eq!(
        compare_text(b"a", None, side(&over_limit), false),
        Err(Code::LimitExceeded)
    );
}

#[test]
fn sensitive_paths_are_gated_before_read_and_never_in_errors() {
    for name in [
        ".env",
        "dir/.ENV.production",
        ".sSh/key",
        "a/.AWS/config",
        ".gnupg/x",
        "CREDENTIALS",
        "secrets/a",
        "id_rsa",
        "a/ID_ED25519",
        "a.PEM",
        "a.key",
        "a.p12",
        "a.pfx",
    ] {
        assert_eq!(
            allow_source_read(name.as_bytes(), false),
            Err(Code::SensitivePath)
        );
        assert!(allow_source_read(name.as_bytes(), true).is_ok());
        let error =
            compare_text(name.as_bytes(), None, side(b"DO-NOT-PRINT-SECRET"), false).unwrap_err();
        let output = Report::failure(vec![Diagnostic::new(error, None)])
            .json()
            .unwrap();
        assert!(!output.contains(name));
        assert!(!output.contains("DO-NOT-PRINT-SECRET"));
    }
    for safe in [
        "environment",
        ".envoy",
        "credentials.rs",
        "secrets.txt",
        "id_rsa.pub",
        "x.pem.rs",
    ] {
        assert!(!sensitive_path(safe));
    }
}

#[test]
fn paths_are_byte_sorted_bounded_and_terminal_safe() {
    let mut paths = CandidatePaths::default();
    for name in [
        "日本語",
        "-dash",
        "space name",
        "a\nb",
        "a\tb",
        "a\\b",
        "\u{202e}bidi",
    ] {
        paths.insert(name.as_bytes()).unwrap();
        paths.insert(name.as_bytes()).unwrap();
        let human = human_path(name);
        assert!(!human.contains('\n'));
        assert!(!human.contains('\t'));
        assert!(!human.contains('\u{202e}'));
    }
    let sorted: Vec<_> = paths.iter().collect();
    assert!(sorted.windows(2).all(|w| w[0].as_bytes() < w[1].as_bytes()));
    for invalid in [
        b"".as_slice(),
        b"/abs",
        b"../escape",
        b"a/../b",
        b"a//b",
        b"a/./b",
        b"a/",
        b".GIT/config",
        b"a\0b",
        b"\xff",
    ] {
        assert!(validate_path(invalid).is_err());
    }
    assert!(validate_path(&vec![b'x'; MAX_PATH_BYTES]).is_ok());
    assert_eq!(
        validate_path(&vec![b'x'; MAX_PATH_BYTES + 1]),
        Err(Code::LimitExceeded)
    );
    assert_eq!(
        human_path("\u{009b}\u{007f}\u{061c}"),
        "\"\\u009b\\u007f\\u061c\""
    );
}

#[test]
fn cumulative_input_and_candidate_limits_include_exact_boundaries() {
    let mut budget = InputBudget::default();
    assert_eq!(budget.reserve(MAX_FILE_BYTES + 1), Err(Code::LimitExceeded));
    for _ in 0..MAX_INPUT_BYTES / MAX_FILE_BYTES {
        budget.reserve(MAX_FILE_BYTES).unwrap();
    }
    assert!(budget.reserve(0).is_ok());
    assert_eq!(budget.reserve(1), Err(Code::LimitExceeded));
    let mut paths = CandidatePaths::default();
    for i in 0..MAX_PATHS {
        paths.insert(i.to_string().as_bytes()).unwrap();
    }
    paths.insert(b"0").unwrap();
    assert_eq!(paths.insert(b"overflow"), Err(Code::LimitExceeded));
}

#[test]
fn deterministic_projection_exit_codes_and_no_contents() {
    let a = target();
    let mut b = target();
    b.build_commit = Some(commit('b'));
    let change = |name: &str| {
        compare_text(name.as_bytes(), None, side(b"PRIVATE-CONTENTS"), false)
            .unwrap()
            .unwrap()
    };
    let first = TargetPreview::new(
        accepted(&a),
        Relationship::Equal,
        vec![change("z"), change("\u{202e}\n")],
    )
    .unwrap();
    let second = TargetPreview::new(accepted(&b), Relationship::Ancestor, vec![]).unwrap();
    let forward = Report::success(vec![first.clone(), second.clone()]).unwrap();
    let reverse = Report::success(vec![second, first]).unwrap();
    assert_eq!(forward.json().unwrap(), reverse.json().unwrap());
    assert_eq!(forward.exit_code(), 1);
    let json = forward.json().unwrap();
    assert!(!json.contains("PRIVATE-CONTENTS"));
    assert!(!json.contains('\u{202e}'));
    assert!(json.contains("\\u202e\\n"));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["filesystem_verified"], false);
    assert_eq!(value["ignored_untracked"], "excluded");
    assert_eq!(value["targets"].as_array().unwrap().len(), 2);
    let clean = Report::success(vec![
        TargetPreview::new(accepted(&a), Relationship::Equal, vec![]).unwrap(),
    ])
    .unwrap();
    assert_eq!(clean.exit_code(), 0);
    let errors = vec![
        Diagnostic::new(Code::SourceChanged, None),
        Diagnostic::new(Code::BaselineUnknown, Some(a.container_id.clone())),
    ];
    let failure = Report::failure(errors.clone());
    assert_eq!(failure.exit_code(), 2);
    assert_eq!(
        failure.json().unwrap(),
        Report::failure(errors.into_iter().rev().collect())
            .json()
            .unwrap()
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&failure.json().unwrap()).unwrap()["targets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(Report::failure(vec![]).exit_code(), 2);
}

#[test]
fn change_target_and_serialized_output_limits() {
    let t = target();
    let changes: Vec<_> = (0..MAX_CHANGES)
        .map(|i| {
            compare_text(i.to_string().as_bytes(), None, side(b""), false)
                .unwrap()
                .unwrap()
        })
        .collect();
    assert!(TargetPreview::new(accepted(&t), Relationship::Equal, changes.clone()).is_ok());
    let mut excess = changes.clone();
    excess.push(changes[0].clone());
    assert_eq!(
        TargetPreview::new(accepted(&t), Relationship::Equal, excess).unwrap_err(),
        Code::LimitExceeded
    );
    assert_eq!(
        TargetPreview::new(
            accepted(&t),
            Relationship::Equal,
            vec![changes[0].clone(), changes[0].clone()]
        )
        .unwrap_err(),
        Code::SourceChanged
    );
    let p = TargetPreview::new(accepted(&t), Relationship::Equal, vec![]).unwrap();
    assert_eq!(
        Report::success(vec![p.clone(), p]).unwrap_err(),
        Code::DiscoveryFailed
    );
    assert_eq!(Report::success(vec![]).unwrap_err(), Code::LimitExceeded);
    let many_targets: Vec<_> = (0..MAX_TARGETS)
        .map(|_| TargetPreview::new(accepted(&target()), Relationship::Equal, vec![]).unwrap())
        .collect();
    assert!(Report::success(many_targets).is_ok());
    // Escaped paths can be larger than input paths: enforce final serialized bytes.
    let huge: Vec<_> = (0..MAX_CHANGES)
        .map(|i| {
            let name = format!("{i:04}{}", "\n".repeat(MAX_PATH_BYTES - 4));
            compare_text(name.as_bytes(), None, side(b""), false)
                .unwrap()
                .unwrap()
        })
        .collect();
    let report = Report::success(vec![
        TargetPreview::new(accepted(&t), Relationship::Equal, huge).unwrap(),
    ])
    .unwrap();
    assert_eq!(report.json(), Err(Code::LimitExceeded));
}
