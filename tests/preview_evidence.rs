#![cfg(unix)]
use bootleg_cli::preview::Code;
use bootleg_cli::preview_evidence::{self as evidence, EntryMode};
use bootleg_cli::preview_git::{HistoryCommand, HistoryGit, ObjectId};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "fixture Git failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(dir.path(), &["config", "user.name", "Test"]);
    std::fs::write(dir.path().join("file"), "first\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "first"]);
    dir
}

fn run(dir: &Path, operation: HistoryCommand<'_>) -> Vec<u8> {
    HistoryGit::new("/usr/bin/git".into())
        .unwrap()
        .run(dir, operation, Instant::now() + Duration::from_secs(10))
        .unwrap()
        .bytes
}

#[test]
fn real_tree_and_index_preserve_modes_names_and_raw_object_identity() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = repository();
    let unusual = "-tab\tline\n雪";
    std::fs::write(dir.path().join(unusual), "text\n").unwrap();
    std::fs::write(dir.path().join("executable"), "script\n").unwrap();
    std::fs::set_permissions(
        dir.path().join("executable"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink("/nonexistent-secret-target", dir.path().join("link")).unwrap();
    git(dir.path(), &["add", "."]);
    // Add a gitlink without initializing a submodule or contacting any remote.
    let commit = git(dir.path(), &["rev-parse", "HEAD"]);
    git(
        dir.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{commit},module"),
        ],
    );
    git(dir.path(), &["commit", "-qm", "entries"]);
    let head = ObjectId::parse(&git(dir.path(), &["rev-parse", "HEAD"])).unwrap();
    let tree = evidence::tree(&run(dir.path(), HistoryCommand::Tree(&head)), 40).unwrap();
    let index_bytes = std::fs::read(dir.path().join(".git/index")).unwrap();
    let index = evidence::index(&run(dir.path(), HistoryCommand::Index), 40).unwrap();
    assert_eq!(tree, index);
    assert_eq!(tree[unusual].mode, EntryMode::Regular);
    assert_eq!(tree["executable"].mode, EntryMode::Executable);
    assert_eq!(tree["link"].mode, EntryMode::Symlink);
    assert_eq!(tree["module"].mode, EntryMode::Gitlink);
    assert_eq!(
        index_bytes,
        std::fs::read(dir.path().join(".git/index")).unwrap()
    );
    assert!(!dir.path().join(".git/index.lock").exists());
    std::fs::write(dir.path().join("new\nfile"), "new").unwrap();
    assert_eq!(
        evidence::untracked(&run(dir.path(), HistoryCommand::Untracked))
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["new\nfile"]
    );
}

#[test]
fn real_unsafe_index_states_fail_closed_without_index_changes() {
    for flag in [
        "--assume-unchanged",
        "--skip-worktree",
        "intent-to-add",
        "conflict",
    ] {
        let dir = repository();
        match flag {
            "intent-to-add" => {
                std::fs::write(dir.path().join("new"), "new\n").unwrap();
                git(dir.path(), &["add", "-N", "new"]);
            }
            "conflict" => {
                let id = git(dir.path(), &["rev-parse", "HEAD:file"]);
                let mut child = Command::new("/usr/bin/git")
                    .current_dir(dir.path())
                    .args(["update-index", "--index-info"])
                    .stdin(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(
                        format!(
                            "0 {}\tfile\n100644 {id} 1\tfile\n100644 {id} 2\tfile\n",
                            "0".repeat(40)
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                assert!(child.wait().unwrap().success());
            }
            _ => {
                git(dir.path(), &["update-index", flag, "file"]);
            }
        }
        let before = std::fs::read(dir.path().join(".git/index")).unwrap();
        assert_eq!(
            evidence::index(&run(dir.path(), HistoryCommand::Index), 40),
            Err(Code::UnsupportedRepository),
            "{flag}"
        );
        assert_eq!(
            before,
            std::fs::read(dir.path().join(".git/index")).unwrap()
        );
        assert!(!dir.path().join(".git/index.lock").exists());
    }
}

#[test]
fn real_sha256_objects_require_matching_width() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "--object-format=sha256"]);
    std::fs::write(dir.path().join("file"), "sha256\n").unwrap();
    git(dir.path(), &["add", "file"]);
    let tree_id = ObjectId::parse(&git(dir.path(), &["write-tree"])).unwrap();
    let bytes = run(dir.path(), HistoryCommand::Tree(&tree_id));
    let tree = evidence::tree(&bytes, 64).unwrap();
    assert_eq!(tree["file"].object.as_str().len(), 64);
    assert_eq!(
        tree,
        evidence::index(&run(dir.path(), HistoryCommand::Index), 64).unwrap()
    );
    assert_eq!(evidence::tree(&bytes, 40), Err(Code::GitFailed));
}

#[test]
fn index_parser_rejects_truncated_unknown_and_duplicate_evidence() {
    let dir = repository();
    let bytes = run(dir.path(), HistoryCommand::Index);
    for end in 1..bytes.len() {
        assert!(evidence::index(&bytes[..end], 40).is_err());
    }
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(evidence::index(text.replace("  ctime:", "  other:").as_bytes(), 40).is_err());
    let mut duplicate = bytes.clone();
    duplicate.extend_from_slice(&bytes);
    assert_eq!(evidence::index(&duplicate, 40), Err(Code::GitFailed));
    assert_eq!(evidence::index(&bytes, 64), Err(Code::GitFailed));
}
