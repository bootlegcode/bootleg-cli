//! Fail-closed Unix capture adapter. No output is emitted until all gates pass.
use crate::{
    preview::{self, Change, Code, InputBudget, Mode, TextSide},
    preview_application::{BaselineEvidence, CapturedSource, SourceCapture},
    preview_evidence::{self as evidence, Entry, EntryMode},
    preview_fs::{self, Root, SourceFile},
    preview_git::{GitError, HistoryCommand as Op, HistoryGit, ObjectId},
};
use bootleg_core::BuildCommit;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Instant,
};

fn git_error(error: GitError) -> Code {
    match error {
        GitError::Deadline | GitError::Limit => Code::LimitExceeded,
        _ => Code::GitFailed,
    }
}
fn id(value: &str) -> Result<ObjectId, Code> {
    ObjectId::parse(value).map_err(git_error)
}
fn line(bytes: &[u8]) -> Result<&str, Code> {
    std::str::from_utf8(bytes.strip_suffix(b"\n").ok_or(Code::GitFailed)?)
        .map_err(|_| Code::GitFailed)
}
fn regular(mode: EntryMode) -> Result<Mode, Code> {
    match mode {
        EntryMode::Regular => Ok(Mode::Regular),
        EntryMode::Executable => Ok(Mode::Executable),
        _ => Err(Code::UnsupportedEntry),
    }
}
fn safe_config(bytes: &[u8]) -> Result<(), Code> {
    if bytes.is_empty() {
        return Ok(());
    }
    let bytes = bytes.strip_suffix(&[0]).ok_or(Code::GitFailed)?;
    for row in bytes.split(|&b| b == 0) {
        let row = std::str::from_utf8(row).map_err(|_| Code::GitFailed)?;
        let (key, value) = row.split_once('\n').ok_or(Code::GitFailed)?;
        if key == "extensions.partialclone"
            || !matches!(
                value.to_ascii_lowercase().as_str(),
                "false" | "no" | "off" | "0"
            )
        {
            return Err(Code::UnsupportedRepository);
        }
    }
    Ok(())
}
fn attributes(bytes: &[u8], path: &str) -> Result<bool, Code> {
    let mut parts = bytes.split(|&b| b == 0);
    let mut unsupported = false;
    for name in ["filter", "working-tree-encoding", "diff"] {
        if parts.next() != Some(path.as_bytes()) || parts.next() != Some(name.as_bytes()) {
            return Err(Code::GitFailed);
        }
        let value = parts.next().ok_or(Code::GitFailed)?;
        unsupported |= !matches!(value, b"unspecified" | b"unset");
    }
    if parts.next() != Some(b"".as_slice()) || parts.next().is_some() {
        return Err(Code::GitFailed);
    }
    Ok(unsupported)
}

pub struct Capture {
    directory: PathBuf,
    git: HistoryGit,
    #[cfg(test)]
    before_verify: Option<Box<dyn FnOnce()>>,
}
pub struct Snapshot {
    head: BuildCommit,
    changes: BTreeMap<String, Vec<Change>>,
}
impl Capture {
    pub fn new(directory: PathBuf, git: PathBuf) -> Result<Self, Code> {
        Ok(Self {
            directory,
            git: HistoryGit::new(git).map_err(git_error)?,
            #[cfg(test)]
            before_verify: None,
        })
    }
    fn run(&self, root: &Path, op: Op<'_>, deadline: Instant) -> Result<Vec<u8>, Code> {
        self.git
            .run(root, op, deadline)
            .map(|o| o.bytes)
            .map_err(git_error)
    }
}
impl CapturedSource for Snapshot {
    fn head(&self) -> Option<&BuildCommit> {
        Some(&self.head)
    }
    fn evidence(&self, baseline: &BuildCommit) -> Result<BaselineEvidence, Code> {
        if !self.changes.contains_key(baseline.as_str()) {
            return Err(Code::MissingObject);
        }
        Ok(BaselineEvidence {
            required_objects_present: true,
            ancestor_of_head: Some(true),
        })
    }
    fn compare(&self, baseline: &BuildCommit, deadline: Instant) -> Result<Vec<Change>, Code> {
        preview_fs::deadline(deadline)?;
        self.changes
            .get(baseline.as_str())
            .cloned()
            .ok_or(Code::MissingObject)
    }
}
impl SourceCapture for Capture {
    type Snapshot = Snapshot;
    fn capture(&mut self, baselines: &[BuildCommit], deadline: Instant) -> Result<Snapshot, Code> {
        if baselines.is_empty() || baselines.len() > preview::MAX_TARGETS {
            return Err(Code::LimitExceeded);
        }
        let root = PathBuf::from(line(&self.run(&self.directory, Op::Root, deadline)?)?);
        let root = root.canonicalize().map_err(|_| Code::IoFailed)?;
        let source = Root::open(&root)?;
        let root_stamp = source.root_stamp()?;
        let gitdir_bytes = self.run(&root, Op::GitDirectory, deadline)?;
        let gitdir = PathBuf::from(line(&gitdir_bytes)?);
        // v1 rejects linked worktrees, separate Git directories and symlinked
        // administration rather than treating an unchecked layout as complete.
        if gitdir != root.join(".git") {
            return Err(Code::UnsupportedRepository);
        }
        let admin = Root::open(&gitdir).map_err(|_| Code::UnsupportedRepository)?;
        let admin_paths = [
            "index",
            "HEAD",
            "config",
            "info/exclude",
            "info/attributes",
            "info/sparse-checkout",
            "shallow",
            "objects/info/alternates",
        ];
        let admin_before = admin_paths
            .iter()
            .map(|p| admin.evidence(p))
            .collect::<Result<Vec<_>, _>>()?;
        let config = self.run(&root, Op::SafetyConfig, deadline)?;
        safe_config(&config)?;
        // Alternate object stores can point outside this repository and race
        // independently. Refuse them until their capture semantics are supported.
        if gitdir.join("objects/info/alternates").exists() {
            return Err(Code::UnsupportedRepository);
        }
        let format = self.run(&root, Op::ObjectFormat, deadline)?;
        let width = match format.as_slice() {
            b"sha1\n" => 40,
            b"sha256\n" => 64,
            _ => return Err(Code::UnsupportedRepository),
        };
        let head_bytes = self.run(&root, Op::Head, deadline)?;
        let head = line(&head_bytes)?
            .parse::<BuildCommit>()
            .map_err(|_| Code::UnsupportedRepository)?;
        let head_id = id(head.as_str())?;
        let index_bytes = self.run(&root, Op::Index, deadline)?;
        let index = evidence::index(&index_bytes, width)?;
        let untracked_bytes = self.run(&root, Op::Untracked, deadline)?;
        let untracked = evidence::untracked(&untracked_bytes)?;
        let head_tree = evidence::tree(&self.run(&root, Op::Tree(&head_id), deadline)?, width)?;
        let mut paths = preview::CandidatePaths::default();
        for path in index.keys().chain(head_tree.keys()).chain(untracked.iter()) {
            paths.insert(path.as_bytes())?;
        }
        let mut trees = BTreeMap::<String, BTreeMap<String, Entry>>::new();
        for baseline in baselines {
            preview_fs::deadline(deadline)?;
            if baseline.as_str().len() != width {
                return Err(Code::MissingObject);
            }
            let baseline_id = id(baseline.as_str())?;
            self.run(&root, Op::CommitExists(&baseline_id), deadline)
                .map_err(|e| {
                    if e == Code::GitFailed {
                        Code::MissingObject
                    } else {
                        e
                    }
                })?;
            if !self
                .git
                .run(&root, Op::IsAncestor(&baseline_id, &head_id), deadline)
                .map_err(git_error)?
                .affirmative
            {
                return Err(Code::UnsafeHistory);
            }
            let tree = evidence::tree(&self.run(&root, Op::Tree(&baseline_id), deadline)?, width)?;
            for path in tree.keys() {
                paths.insert(path.as_bytes())?;
            }
            trees.insert(baseline.as_str().to_owned(), tree);
        }
        // Refuse gitlinks before directory inspection; never enter submodules.
        for entry in index
            .values()
            .chain(head_tree.values())
            .chain(trees.values().flat_map(|tree| tree.values()))
        {
            regular(entry.mode)?;
        }
        let special_gate = |source: &Root| {
            source.special_gate(
                |path| {
                    self.git
                        .run(&root, Op::Ignored(path), deadline)
                        .map(|o| o.affirmative)
                        .map_err(git_error)
                },
                deadline,
            )
        };
        let directory_stamps = special_gate(&source)?;
        let mut budget = InputBudget::default();
        let mut files = BTreeMap::<String, Option<SourceFile>>::new();
        let mut stamps = BTreeMap::new();
        for path in paths.iter() {
            preview_fs::deadline(deadline)?;
            // Conservative pre-read gate: no stat-cache equality shortcut.
            preview::allow_source_read(path.as_bytes(), false)?;
            for entry in index
                .get(path)
                .into_iter()
                .chain(head_tree.get(path))
                .chain(trees.values().filter_map(|t| t.get(path)))
            {
                regular(entry.mode)?;
            }
            let (file, stamp) = source.read(path, &mut budget, deadline)?;
            files.insert(path.to_owned(), file);
            stamps.insert(path.to_owned(), stamp);
        }
        let mut blobs = BTreeMap::<String, Vec<u8>>::new();
        let mut attrs = BTreeMap::<String, Vec<u8>>::new();
        let mut changes = BTreeMap::new();
        for (baseline, tree) in &trees {
            let mut result = Vec::new();
            for path in paths.iter() {
                preview_fs::deadline(deadline)?;
                let entry = tree.get(path);
                if let Some(entry) = entry {
                    let key = entry.object.as_str();
                    if !blobs.contains_key(key) {
                        let size = self
                            .run(&root, Op::BlobSize(&entry.object), deadline)
                            .map_err(|e| {
                                if e == Code::GitFailed {
                                    Code::MissingObject
                                } else {
                                    e
                                }
                            })?;
                        let size = line(&size)?.parse::<usize>().map_err(|_| Code::GitFailed)?;
                        budget.reserve(size)?;
                        let bytes =
                            self.run(&root, Op::BlobWithin(&entry.object, size), deadline)?;
                        if bytes.len() != size {
                            return Err(Code::SourceChanged);
                        }
                        blobs.insert(key.to_owned(), bytes);
                    }
                }
                let old = entry
                    .map(|e| {
                        Ok(TextSide {
                            mode: regular(e.mode)?,
                            bytes: &blobs[e.object.as_str()],
                        })
                    })
                    .transpose()?;
                let new = files[path].as_ref().map(|f| TextSide {
                    mode: f.mode,
                    bytes: &f.bytes,
                });
                if old.map(|s| (s.mode, s.bytes)) == new.map(|s| (s.mode, s.bytes)) {
                    continue;
                }
                if !attrs.contains_key(path) {
                    attrs.insert(
                        path.to_owned(),
                        self.run(&root, Op::Attributes(path), deadline)?,
                    );
                }
                if let Some(change) = preview::compare_text(
                    path.as_bytes(),
                    old,
                    new,
                    attributes(&attrs[path], path)?,
                )? {
                    if result.len() == preview::MAX_CHANGES {
                        return Err(Code::LimitExceeded);
                    }
                    result.push(change);
                }
            }
            changes.insert(baseline.clone(), result);
        }
        // All comparisons share the captured bytes; verify source and Git
        // evidence before returning any successful target.
        #[cfg(test)]
        if let Some(hook) = self.before_verify.take() {
            hook();
        }
        let reopened = Root::open(&root)?;
        if source.root_stamp()? != root_stamp || reopened.root_stamp()? != root_stamp {
            return Err(Code::SourceChanged);
        }
        let reopened_admin = Root::open(&gitdir)?;
        if special_gate(&reopened)? != directory_stamps {
            return Err(Code::SourceChanged);
        }
        for (path, before) in &stamps {
            preview_fs::deadline(deadline)?;
            if source.evidence(path)? != *before || reopened.evidence(path)? != *before {
                return Err(Code::SourceChanged);
            }
        }
        for (path, before) in admin_paths.iter().zip(admin_before) {
            if admin.evidence(path)? != before || reopened_admin.evidence(path)? != before {
                return Err(Code::SourceChanged);
            }
        }
        for (op, before) in [
            (Op::Head, &head_bytes),
            (Op::Index, &index_bytes),
            (Op::Untracked, &untracked_bytes),
            (Op::SafetyConfig, &config),
            (Op::GitDirectory, &gitdir_bytes),
        ] {
            if self.run(&root, op, deadline)? != *before {
                return Err(Code::SourceChanged);
            }
        }
        for (path, before) in &attrs {
            if self.run(&root, Op::Attributes(path), deadline)? != *before {
                return Err(Code::SourceChanged);
            }
        }
        preview_fs::deadline(deadline)?;
        Ok(Snapshot { head, changes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_index_races_discard_the_entire_capture() {
        for changed in ["file", ".git/index"] {
            let dir = tempfile::tempdir().unwrap();
            let git = |args: &[&str]| {
                let result = std::process::Command::new("/usr/bin/git")
                    .current_dir(dir.path())
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .args(args)
                    .output()
                    .unwrap();
                assert!(result.status.success());
                String::from_utf8(result.stdout).unwrap().trim().to_owned()
            };
            git(&["init", "-q"]);
            git(&["config", "user.email", "test@example.invalid"]);
            git(&["config", "user.name", "Test"]);
            std::fs::write(dir.path().join("file"), "original\n").unwrap();
            git(&["add", "."]);
            git(&["commit", "-qm", "initial"]);
            let baseline = git(&["rev-parse", "HEAD"]).parse().unwrap();
            let mut capture = Capture::new(dir.path().to_owned(), "/usr/bin/git".into()).unwrap();
            let path = dir.path().join(changed);
            // This hook exists only in test builds, after comparisons and before
            // the production revalidation boundary; no scheduling race needed.
            capture.before_verify = Some(Box::new(move || {
                std::fs::write(path, "changed after capture\n").unwrap()
            }));
            assert!(matches!(
                capture.capture(
                    &[baseline],
                    Instant::now() + std::time::Duration::from_secs(10)
                ),
                Err(Code::SourceChanged)
            ));
        }
    }
}
