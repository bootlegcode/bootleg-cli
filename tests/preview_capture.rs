#![cfg(unix)]
use bootleg_cli::{
    preview::{Code, InputBudget},
    preview_application::{CapturedSource, SourceCapture},
    preview_capture::Capture,
    preview_fs::Root,
};
use bootleg_core::BuildCommit;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("/usr/bin/git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "fixture failure");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn repo() -> (tempfile::TempDir, BuildCommit) {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["config", "user.email", "test@example.invalid"]);
    git(d.path(), &["config", "user.name", "Test"]);
    std::fs::write(d.path().join("file"), "first\r\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "first"]);
    let head = git(d.path(), &["rev-parse", "HEAD"]).parse().unwrap();
    (d, head)
}
fn capture(
    dir: &Path,
    bases: &[BuildCommit],
) -> Result<bootleg_cli::preview_capture::Snapshot, Code> {
    Capture::new(dir.to_owned(), "/usr/bin/git".into())
        .unwrap()
        .capture(bases, Instant::now() + Duration::from_secs(30))
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, p: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in std::fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                out.insert(p.strip_prefix(root).unwrap().to_owned(), vec![]);
                visit(root, &p, out);
            } else if e.file_type().unwrap().is_symlink() {
                out.insert(
                    p.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read_link(p)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec(),
                );
            } else if e.file_type().unwrap().is_file() {
                out.insert(
                    p.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(p).unwrap(),
                );
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_owned(),
                    b"special".to_vec(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
#[test]
fn final_worktree_multiple_baselines_are_deterministic_and_read_only() {
    let (d, first) = repo();
    std::fs::write(d.path().join(".gitignore"), "ignored\n").unwrap();
    std::fs::write(d.path().join("file"), "committed\n").unwrap();
    std::fs::write(d.path().join("gone"), "delete\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "second"]);
    let second: BuildCommit = git(d.path(), &["rev-parse", "HEAD"]).parse().unwrap();
    std::fs::write(d.path().join("file"), "staged\n").unwrap();
    git(d.path(), &["add", "file"]);
    // Staged edit is not desired content.
    std::fs::write(d.path().join("file"), "first\r\n").unwrap();
    std::fs::rename(d.path().join("gone"), d.path().join("new\tline\n雪")).unwrap();
    std::fs::write(d.path().join("ignored"), "ignored secret").unwrap();
    let before = snapshot(d.path());
    let bases = [first.clone(), second.clone()];
    let a = capture(d.path(), &bases).unwrap();
    let b = capture(d.path(), &bases).unwrap();
    let get = |s: &bootleg_cli::preview_capture::Snapshot, base: &BuildCommit| {
        serde_json::to_value(
            s.compare(base, Instant::now() + Duration::from_secs(5))
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(get(&a, &first), get(&b, &first));
    assert_eq!(get(&a, &second), get(&b, &second));
    let old = get(&a, &first);
    let new = get(&a, &second);
    assert!(!old.as_array().unwrap().iter().any(|v| v["path"] == "file"));
    assert!(
        new.as_array()
            .unwrap()
            .iter()
            .any(|v| v["path"] == "file" && v["kind"] == "modify")
    );
    assert!(
        new.as_array()
            .unwrap()
            .iter()
            .any(|v| v["path"] == "gone" && v["kind"] == "delete")
    );
    assert!(
        new.as_array()
            .unwrap()
            .iter()
            .any(|v| v["path"] == "new\tline\n雪" && v["kind"] == "add")
    );
    assert!(!format!("{old}{new}").contains("secret"));
    assert_eq!(before, snapshot(d.path()));
}
#[test]
fn equal_detached_missing_and_divergent_history() {
    let (d, base) = repo();
    git(d.path(), &["checkout", "--detach", "-q"]);
    assert!(
        capture(d.path(), std::slice::from_ref(&base))
            .unwrap()
            .compare(&base, Instant::now() + Duration::from_secs(5))
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        capture(d.path(), &["0".repeat(40).parse().unwrap()]),
        Err(Code::MissingObject)
    ));
    git(d.path(), &["commit", "--allow-empty", "-qm", "descendant"]);
    let descendant: BuildCommit = git(d.path(), &["rev-parse", "HEAD"]).parse().unwrap();
    git(d.path(), &["checkout", "--detach", "-q", base.as_str()]);
    git(d.path(), &["commit", "--allow-empty", "-qm", "divergent"]);
    assert!(matches!(
        capture(d.path(), &[descendant]),
        Err(Code::UnsafeHistory)
    ));
}
#[test]
fn unsupported_changes_and_sensitive_paths_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for scenario in [
        "binary",
        "mode",
        "symlink",
        "sensitive",
        "attributes",
        "oversize",
        "sparse",
        "promisor",
        "gitlink",
        "fifo",
    ] {
        let (d, base) = repo();
        let expected = match scenario {
            "binary" => {
                std::fs::write(d.path().join("file"), b"a\0b").unwrap();
                Code::UnsupportedEntry
            }
            "mode" => {
                std::fs::set_permissions(
                    d.path().join("file"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
                Code::UnsupportedEntry
            }
            "symlink" => {
                std::fs::remove_file(d.path().join("file")).unwrap();
                symlink("/etc/passwd", d.path().join("file")).unwrap();
                Code::UnsupportedEntry
            }
            "sensitive" => {
                std::fs::write(d.path().join(".env"), "super-secret").unwrap();
                Code::SensitivePath
            }
            "attributes" => {
                std::fs::write(d.path().join(".gitattributes"), "file filter=secret\n").unwrap();
                std::fs::write(d.path().join("file"), "changed").unwrap();
                Code::UnsupportedAttributes
            }
            "oversize" => {
                std::fs::write(d.path().join("file"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
                Code::LimitExceeded
            }
            "sparse" => {
                git(d.path(), &["config", "core.sparseCheckout", "true"]);
                Code::UnsupportedRepository
            }
            "promisor" => {
                git(d.path(), &["config", "remote.origin.promisor", "true"]);
                Code::UnsupportedRepository
            }
            "gitlink" => {
                git(
                    d.path(),
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("160000,{},module", base.as_str()),
                    ],
                );
                std::fs::create_dir(d.path().join("module")).unwrap();
                std::fs::write(d.path().join("module/dirty-untracked"), "must not traverse")
                    .unwrap();
                Code::UnsupportedEntry
            }
            "fifo" => {
                assert!(
                    Command::new("/usr/bin/mkfifo")
                        .arg(d.path().join("pipe"))
                        .status()
                        .unwrap()
                        .success()
                );
                Code::UnsupportedEntry
            }
            _ => unreachable!(),
        };
        let before = snapshot(d.path());
        let error = capture(d.path(), &[base]).err();
        assert_eq!(error, Some(expected), "{scenario}");
        assert_eq!(before, snapshot(d.path()));
    }
}
#[test]
fn special_file_gate_honors_ignored_directories_and_literal_directory_names() {
    let (d, base) = repo();
    std::fs::create_dir_all(d.path().join(":(literal)dir/deep")).unwrap();
    std::fs::write(d.path().join(":(literal)dir/deep/file"), "new\n").unwrap();
    std::fs::create_dir(d.path().join("ignored")).unwrap();
    std::fs::write(d.path().join(".gitignore"), "ignored/\nignored-pipe\n").unwrap();
    for path in ["ignored/pipe", "ignored-pipe"] {
        assert!(
            Command::new("/usr/bin/mkfifo")
                .arg(d.path().join(path))
                .status()
                .unwrap()
                .success()
        );
    }
    assert!(capture(d.path(), std::slice::from_ref(&base)).is_ok());
    assert!(
        Command::new("/usr/bin/mkfifo")
            .arg(d.path().join(":(literal)dir/deep/pipe"))
            .status()
            .unwrap()
            .success()
    );
    assert!(matches!(
        capture(d.path(), &[base]),
        Err(Code::UnsupportedEntry)
    ));
}

#[test]
fn cumulative_capture_budget_and_missing_blob_are_fail_closed() {
    let (d, base) = repo();
    for n in 0..17 {
        std::fs::write(
            d.path().join(format!("large-{n:02}")),
            vec![b'x'; 1024 * 1024],
        )
        .unwrap();
    }
    assert!(matches!(
        capture(d.path(), std::slice::from_ref(&base)),
        Err(Code::LimitExceeded)
    ));
    for n in 0..17 {
        std::fs::remove_file(d.path().join(format!("large-{n:02}"))).unwrap();
    }
    let blob = git(d.path(), &["rev-parse", "HEAD:file"]);
    std::fs::remove_file(
        d.path()
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    assert!(matches!(
        capture(d.path(), &[base]),
        Err(Code::MissingObject)
    ));
}

#[test]
fn replacement_objects_are_ignored_and_shallow_missing_history_is_rejected() {
    let (d, base) = repo();
    std::fs::write(d.path().join("file"), "replacement\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "replacement"]);
    let later: BuildCommit = git(d.path(), &["rev-parse", "HEAD"]).parse().unwrap();
    git(d.path(), &["replace", base.as_str(), later.as_str()]);
    let captured = capture(d.path(), std::slice::from_ref(&base)).unwrap();
    assert_eq!(
        captured
            .compare(&base, Instant::now() + Duration::from_secs(5))
            .unwrap()
            .len(),
        1
    );
    let shallow_parent = tempfile::tempdir().unwrap();
    let shallow = shallow_parent.path().join("shallow");
    git(
        shallow_parent.path(),
        &[
            "clone",
            "--quiet",
            "--depth=1",
            &format!("file://{}", d.path().display()),
            shallow.to_str().unwrap(),
        ],
    );
    assert!(capture(&shallow, &[later]).is_ok());
    assert!(matches!(
        capture(&shallow, &[base]),
        Err(Code::MissingObject)
    ));
}

#[test]
fn descriptor_reads_reject_parent_symlinks_and_detect_replacement() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("secret"), "must not read").unwrap();
    symlink(external.path(), d.path().join("redirect")).unwrap();
    let root = Root::open(&d.path().canonicalize().unwrap()).unwrap();
    assert!(
        root.read(
            "redirect/secret",
            &mut InputBudget::default(),
            Instant::now() + Duration::from_secs(1)
        )
        .is_err()
    );
    std::fs::write(d.path().join("safe"), "safe").unwrap();
    let (_, before) = root
        .read(
            "safe",
            &mut InputBudget::default(),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
    std::fs::remove_file(d.path().join("safe")).unwrap();
    symlink(external.path().join("secret"), d.path().join("safe")).unwrap();
    assert_ne!(root.evidence("safe").unwrap(), before);
    assert!(
        root.read(
            "safe",
            &mut InputBudget::default(),
            Instant::now() + Duration::from_secs(1)
        )
        .is_err()
    );
    assert!(
        root.read("safe", &mut InputBudget::default(), Instant::now())
            .is_err()
    );
}
