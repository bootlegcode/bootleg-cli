use crate::domain::{BaselineRelationship, GitEvidence, classify};
use bootleg_core::BuildCommit;
use serde::Serialize;
use std::{path::Path, process::Command};

#[derive(Debug, Serialize)]
pub struct BaselineInspection {
    pub relationship: BaselineRelationship,
    pub build_dirty: Option<bool>,
    pub missing_local_object: bool,
    pub local_tracked_changes: Option<bool>,
    pub local_untracked_changes: Option<bool>,
    pub exact_filesystem_drift: &'static str,
}
fn git(root: &Path, args: &[&str]) -> Option<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .output()
        .ok()
}
fn ancestor(root: &Path, a: &str, b: &str) -> Option<bool> {
    match git(root, &["merge-base", "--is-ancestor", a, b])?
        .status
        .code()?
    {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}
pub fn inspect(
    root: &Path,
    baseline: Option<&BuildCommit>,
    dirty: Option<bool>,
) -> BaselineInspection {
    let head = git(root, &["rev-parse", "--verify", "HEAD"])
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<BuildCommit>().ok());
    let object_present = baseline.is_some_and(|b| {
        git(
            root,
            &["cat-file", "-e", &format!("{}^{{commit}}", b.as_str())],
        )
        .is_some_and(|o| o.status.success())
    });
    let mut e = GitEvidence {
        head,
        object_present,
        ..Default::default()
    };
    if let (Some(b), Some(h)) = (baseline, &e.head) {
        e.baseline_ancestor = ancestor(root, b.as_str(), h.as_str());
        e.head_ancestor = ancestor(root, h.as_str(), b.as_str());
        // A shallow graph cannot prove divergence or non-ancestry across its boundary.
        let shallow = git(root, &["rev-parse", "--is-shallow-repository"])
            .is_some_and(|o| o.stdout == b"true\n");
        if shallow {
            if e.baseline_ancestor == Some(false) {
                e.baseline_ancestor = None;
            }
            if e.head_ancestor == Some(false) {
                e.head_ancestor = None;
            }
        }
    }
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )
    .filter(|o| o.status.success());
    let (tracked, untracked) = status
        .map(|o| {
            let mut tracked = false;
            let mut untracked = false;
            let mut renamed = false;
            for record in o.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
                if renamed {
                    renamed = false;
                    continue;
                }
                if record.starts_with(b"??") {
                    untracked = true;
                } else {
                    tracked = true;
                    renamed = record.first().is_some_and(|b| *b == b'R' || *b == b'C')
                        || record.get(1).is_some_and(|b| *b == b'R' || *b == b'C');
                }
            }
            (Some(tracked), Some(untracked))
        })
        .unwrap_or((None, None));
    BaselineInspection {
        relationship: classify(baseline, &e),
        build_dirty: dirty,
        missing_local_object: baseline.is_some() && !object_present,
        local_tracked_changes: tracked,
        local_untracked_changes: untracked,
        exact_filesystem_drift: "unknown",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn run(root: &Path, args: &[&str]) {
        assert!(git(root, args).unwrap().status.success());
    }
    #[test]
    fn real_git_edges() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        assert!(inspect(r, None, None).local_tracked_changes.is_none());
        run(r, &["init", "-q"]);
        run(r, &["config", "user.email", "test@example.test"]);
        run(r, &["config", "user.name", "Test"]);
        assert_eq!(
            inspect(r, None, None).relationship,
            BaselineRelationship::Unknown
        );
        std::fs::write(r.join("file"), "one").unwrap();
        run(r, &["add", "."]);
        run(r, &["commit", "-qm", "one"]);
        let sha: BuildCommit = String::from_utf8(git(r, &["rev-parse", "HEAD"]).unwrap().stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let clean = inspect(r, Some(&sha), Some(false));
        assert_eq!(clean.relationship, BaselineRelationship::Equal);
        assert_eq!(clean.local_tracked_changes, Some(false));
        std::fs::write(r.join("file"), "two").unwrap();
        std::fs::write(r.join("new"), "new").unwrap();
        let dirty = inspect(r, Some(&sha), Some(true));
        assert_eq!(dirty.local_tracked_changes, Some(true));
        assert_eq!(dirty.local_untracked_changes, Some(true));
        assert_eq!(dirty.exact_filesystem_drift, "unknown");
        run(r, &["add", "."]);
        run(r, &["commit", "-qm", "two"]);
        assert_eq!(
            inspect(r, Some(&sha), None).relationship,
            BaselineRelationship::Ancestor
        );
        assert!(inspect(r, Some(&"a".repeat(40).parse().unwrap()), None).missing_local_object);
        let second: BuildCommit = String::from_utf8(git(r, &["rev-parse", "HEAD"]).unwrap().stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        run(r, &["checkout", "--detach", sha.as_str()]);
        assert_eq!(
            inspect(r, Some(&second), None).relationship,
            BaselineRelationship::Descendant
        );
        std::fs::write(r.join("file"), "branch").unwrap();
        run(r, &["add", "."]);
        run(r, &["commit", "-qm", "divergent"]);
        assert_eq!(
            inspect(r, Some(&second), None).relationship,
            BaselineRelationship::Diverged
        );
        // A shallow boundary cannot prove divergence, even when both objects exist.
        let head = String::from_utf8(git(r, &["rev-parse", "HEAD"]).unwrap().stdout).unwrap();
        std::fs::write(r.join(".git/shallow"), head).unwrap();
        assert_eq!(
            inspect(r, Some(&second), None).relationship,
            BaselineRelationship::Unknown
        );
    }
}
