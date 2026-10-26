//! Read-only preview v1 domain policy. No subprocess, filesystem, or API access.
//!
//! Used by the `diff` command. Adapters capture one bounded source view,
//! enforce the pre-read sensitive-path gate, and establish Git evidence.
use bootleg_core::{BuildCommit, ContainerId};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_TARGETS: usize = 32;
pub const MAX_PATHS: usize = 100_000;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_CHANGES: usize = 1000;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub const DISCLAIMER: &str = "Preview relative to the reported build commit; container filesystem contents are not verified.";

/// Only safe static diagnostics can reach the output projection.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    BaselineDirty,
    BaselineUnknown,
    DiscoveryFailed,
    GitFailed,
    IoFailed,
    LimitExceeded,
    MissingObject,
    SensitivePath,
    SourceChanged,
    TargetUnavailable,
    UnsafeHistory,
    UnsupportedAttributes,
    UnsupportedEntry,
    UnsupportedRepository,
}
impl Code {
    fn message(self) -> &'static str {
        match self {
            Self::BaselineDirty => "Reported build baseline is dirty.",
            Self::BaselineUnknown => "Reported build baseline is unknown.",
            Self::DiscoveryFailed => "Authorized discovery could not be completed.",
            Self::GitFailed => "Read-only Git inspection failed.",
            Self::IoFailed => "Read-only preview I/O failed.",
            Self::LimitExceeded => "A preview safety limit was exceeded.",
            Self::MissingObject => "A required local Git object is unavailable.",
            Self::SensitivePath => "A candidate path is excluded by the sensitive-path policy.",
            Self::SourceChanged => {
                "Local source changed during capture; stop local writers and retry."
            }
            Self::TargetUnavailable => "A selected target is unavailable in authorized discovery.",
            Self::UnsafeHistory => {
                "The reported baseline is not provably equal to or an ancestor of HEAD."
            }
            Self::UnsupportedAttributes => "A changed entry has unsupported Git attributes.",
            Self::UnsupportedEntry => "A changed source entry is unsupported.",
            Self::UnsupportedRepository => "The repository cannot be safely enumerated.",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub container_id: ContainerId,
    pub build_commit: Option<BuildCommit>,
    pub build_dirty: Option<bool>,
    pub connected: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    Equal,
    Ancestor,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedTarget {
    pub container_id: ContainerId,
    pub build_commit: BuildCommit,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    code: Code,
    container_id: Option<ContainerId>,
    message: &'static str,
}
impl Diagnostic {
    pub fn new(code: Code, container_id: Option<ContainerId>) -> Self {
        Self {
            code,
            container_id,
            message: code.message(),
        }
    }
}

/// Input must be the *complete* bounded authorized traversal, not a first page.
/// Duplicate selected records must agree on every frozen preview-relevant field.
pub fn select_targets(
    requested: &[ContainerId],
    discovered: &[Target],
) -> Result<Vec<AcceptedTarget>, Vec<Diagnostic>> {
    let ids: BTreeMap<_, _> = requested.iter().map(|id| (id.to_string(), id)).collect();
    if ids.is_empty() || ids.len() > MAX_TARGETS {
        return Err(vec![Diagnostic::new(Code::LimitExceeded, None)]);
    }
    let mut accepted = Vec::new();
    let mut errors = Vec::new();
    for id in ids.values() {
        let mut records = discovered.iter().filter(|t| t.container_id == **id);
        let target = records.next();
        let code = match target {
            None => Some(Code::TargetUnavailable),
            Some(t) if records.any(|other| other != t) => Some(Code::DiscoveryFailed),
            Some(t) if !t.connected => Some(Code::TargetUnavailable),
            Some(t) if t.build_dirty == Some(true) => Some(Code::BaselineDirty),
            Some(t) if t.build_commit.is_none() || t.build_dirty.is_none() => {
                Some(Code::BaselineUnknown)
            }
            Some(_) => None,
        };
        if let Some(code) = code {
            errors.push(Diagnostic::new(code, Some((*id).clone())));
        } else {
            let t = target.expect("validated target");
            accepted.push(AcceptedTarget {
                container_id: t.container_id.clone(),
                build_commit: t.build_commit.clone().expect("validated commit"),
            });
        }
    }
    if errors.is_empty() {
        Ok(accepted)
    } else {
        Err(errors)
    }
}

/// Evidence must refer to one captured HEAD, with replacement objects disabled.
/// A true ancestry proof is valid even in a shallow repository; false/unknown is not.
pub fn relationship(
    baseline: &BuildCommit,
    head: Option<&BuildCommit>,
    commit_and_required_objects_present: bool,
    baseline_ancestor: Option<bool>,
) -> Result<Relationship, Code> {
    let head = head.ok_or(Code::UnsupportedRepository)?;
    if !commit_and_required_objects_present || baseline.as_str().len() != head.as_str().len() {
        return Err(Code::MissingObject);
    }
    if baseline == head {
        return Ok(Relationship::Equal);
    }
    if baseline_ancestor == Some(true) {
        Ok(Relationship::Ancestor)
    } else {
        Err(Code::UnsafeHistory)
    }
}

/// Validate before using a path in filesystem or Git arguments. Git adapter must
/// supply root-relative UTF-8 paths from NUL-delimited plumbing.
pub fn validate_path(path: &[u8]) -> Result<&str, Code> {
    if path.len() > MAX_PATH_BYTES {
        return Err(Code::LimitExceeded);
    }
    let path = std::str::from_utf8(path).map_err(|_| Code::UnsupportedEntry)?;
    if path.is_empty()
        || path.contains('\0')
        || path
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || c.eq_ignore_ascii_case(".git"))
    {
        return Err(Code::UnsupportedEntry);
    }
    Ok(path)
}
pub fn sensitive_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.split('/').any(|c| {
        matches!(
            c,
            ".env" | ".ssh" | ".aws" | ".gnupg" | "credentials" | "secrets"
        ) || c.starts_with(".env.")
    }) || path.rsplit('/').next().is_some_and(|name| {
        matches!(name, "id_rsa" | "id_ed25519")
            || [".pem", ".key", ".p12", ".pfx"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
    })
}

/// Call *before* reading source bytes. Sensitive entries may only be skipped when
/// an adapter already proved them unchanged without opening worktree contents.
/// Otherwise fail closed, even if they might ultimately compare equal.
pub fn allow_source_read(path: &[u8], proven_unchanged: bool) -> Result<&str, Code> {
    let path = validate_path(path)?;
    if !proven_unchanged && sensitive_path(path) {
        return Err(Code::SensitivePath);
    }
    Ok(path)
}

/// Mode is kept typed so arbitrary strings cannot reach the output.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum Mode {
    #[serde(rename = "100644")]
    Regular,
    #[serde(rename = "100755")]
    Executable,
}
#[derive(Clone, Copy)]
pub struct TextSide<'a> {
    pub mode: Mode,
    pub bytes: &'a [u8],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Add,
    Modify,
    Delete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Change {
    path: String,
    kind: Kind,
    old_bytes: Option<usize>,
    new_bytes: Option<usize>,
    old_mode: Option<Mode>,
    new_mode: Option<Mode>,
}

/// Only regular files reach this policy. Adapters reject changed symlinks,
/// gitlinks, special files and type changes without following or recursing them.
/// Unsupported attributes are a gate only for changed entries.
pub fn compare_text(
    path: &[u8],
    old: Option<TextSide<'_>>,
    new: Option<TextSide<'_>>,
    unsupported_attributes: bool,
) -> Result<Option<Change>, Code> {
    let path = validate_path(path)?;
    if old.map(|s| (s.mode, s.bytes)) == new.map(|s| (s.mode, s.bytes)) {
        return Ok(None);
    }
    // Defense in depth, not a substitute for the adapter's pre-read check.
    if sensitive_path(path) {
        return Err(Code::SensitivePath);
    }
    if unsupported_attributes {
        return Err(Code::UnsupportedAttributes);
    }
    if let (Some(old), Some(new)) = (old, new)
        && old.mode != new.mode
    {
        return Err(Code::UnsupportedEntry);
    }
    for side in [old, new].into_iter().flatten() {
        if side.bytes.len() > MAX_FILE_BYTES {
            return Err(Code::LimitExceeded);
        }
        if side.bytes.contains(&0) || std::str::from_utf8(side.bytes).is_err() {
            return Err(Code::UnsupportedEntry);
        }
    }
    Ok(Some(Change {
        path: path.to_owned(),
        kind: match (old, new) {
            (None, Some(_)) => Kind::Add,
            (Some(_), None) => Kind::Delete,
            (Some(_), Some(_)) => Kind::Modify,
            (None, None) => unreachable!("equal absent sides returned above"),
        },
        old_bytes: old.map(|s| s.bytes.len()),
        new_bytes: new.map(|s| s.bytes.len()),
        old_mode: old.map(|s| s.mode),
        new_mode: new.map(|s| s.mode),
    }))
}

/// Adapters reserve bytes *before allocation*. Charge each read unless reliable
/// object identity proves it was already charged; this type never guesses identity.
#[derive(Default)]
pub struct InputBudget {
    bytes: usize,
}
impl InputBudget {
    pub fn reserve(&mut self, bytes: usize) -> Result<(), Code> {
        if bytes > MAX_FILE_BYTES {
            return Err(Code::LimitExceeded);
        }
        let total = self.bytes.checked_add(bytes).ok_or(Code::LimitExceeded)?;
        if total > MAX_INPUT_BYTES {
            return Err(Code::LimitExceeded);
        }
        self.bytes = total;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetPreview {
    container_id: ContainerId,
    build_commit: BuildCommit,
    relationship: Relationship,
    changes: Vec<Change>,
}
impl TargetPreview {
    pub fn new(
        target: AcceptedTarget,
        relationship: Relationship,
        mut changes: Vec<Change>,
    ) -> Result<Self, Code> {
        if changes.len() > MAX_CHANGES {
            return Err(Code::LimitExceeded);
        }
        changes.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
        if changes.windows(2).any(|w| w[0].path == w[1].path) {
            return Err(Code::SourceChanged);
        }
        Ok(Self {
            container_id: target.container_id,
            build_commit: target.build_commit,
            relationship,
            changes,
        })
    }
}
#[derive(Debug, Serialize)]
pub struct Report {
    schema_version: u8,
    status: &'static str,
    comparison: &'static str,
    filesystem_verified: bool,
    scope: &'static str,
    ignored_untracked: &'static str,
    targets: Vec<TargetPreview>,
    errors: Vec<Diagnostic>,
}
impl Report {
    fn empty(status: &'static str) -> Self {
        Self {
            schema_version: 1,
            status,
            comparison: "reported_build_commit",
            filesystem_verified: false,
            scope: "tracked_and_nonignored_untracked",
            ignored_untracked: "excluded",
            targets: vec![],
            errors: vec![],
        }
    }
    pub fn success(mut targets: Vec<TargetPreview>) -> Result<Self, Code> {
        if targets.is_empty() || targets.len() > MAX_TARGETS {
            return Err(Code::LimitExceeded);
        }
        targets.sort_by_key(|t| t.container_id.to_string());
        if targets
            .windows(2)
            .any(|w| w[0].container_id == w[1].container_id)
        {
            return Err(Code::DiscoveryFailed);
        }
        let mut report = Self::empty("ok");
        report.targets = targets;
        Ok(report)
    }
    pub fn failure(mut errors: Vec<Diagnostic>) -> Self {
        if errors.is_empty() {
            errors.push(Diagnostic::new(Code::IoFailed, None));
        }
        errors.sort_by_key(|e| (e.code, e.container_id.as_ref().map(ToString::to_string)));
        errors.dedup();
        let mut report = Self::empty("error");
        report.errors = errors;
        report
    }
    pub fn exit_code(&self) -> u8 {
        if self.status == "error" {
            2
        } else if self.targets.iter().any(|t| !t.changes.is_empty()) {
            1
        } else {
            0
        }
    }
    /// Plain human rendering follows the same sorted, metadata-only projection.
    pub fn human(&self) -> Result<String, Code> {
        use std::fmt::Write;
        let mut text = format!(
            "{DISCLAIMER}\nScope: tracked and nonignored untracked files; ignored untracked files excluded.\n"
        );
        for error in &self.errors {
            writeln!(text, "{}", error.message).expect("string write");
        }
        for target in &self.targets {
            writeln!(
                text,
                "{} · {}",
                target.container_id,
                target.build_commit.as_str()
            )
            .expect("string write");
            if target.changes.is_empty() {
                text.push_str("  No changes.\n");
            }
            for change in &target.changes {
                let kind = match change.kind {
                    Kind::Add => "add",
                    Kind::Modify => "modify",
                    Kind::Delete => "delete",
                };
                let mode = |m| match m {
                    Some(Mode::Regular) => "100644",
                    Some(Mode::Executable) => "100755",
                    None => "-",
                };
                let bytes = |n: Option<usize>| n.map_or_else(|| "-".to_owned(), |n| n.to_string());
                writeln!(
                    text,
                    "  {kind} {} (bytes: {} -> {}; mode: {} -> {})",
                    human_path(&change.path),
                    bytes(change.old_bytes),
                    bytes(change.new_bytes),
                    mode(change.old_mode),
                    mode(change.new_mode)
                )
                .expect("string write");
                if text.len() > MAX_OUTPUT_BYTES {
                    return Err(Code::LimitExceeded);
                }
            }
        }
        Ok(text)
    }
    /// Validate and buffer before stdout. Escaping bidi controls also prevents
    /// visual reordering when a JSON report is displayed directly in a terminal.
    pub fn json(&self) -> Result<String, Code> {
        let mut text = Vec::new();
        serde_json::to_writer(BoundedWriter(&mut text), self).map_err(|_| Code::LimitExceeded)?;
        let raw = String::from_utf8(text).map_err(|_| Code::IoFailed)?;
        let escaped = escape_bidi(&raw);
        if escaped.len() > MAX_OUTPUT_BYTES {
            return Err(Code::LimitExceeded);
        }
        Ok(escaped)
    }
}
struct BoundedWriter<'a>(&'a mut Vec<u8>);
impl std::io::Write for BoundedWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > MAX_OUTPUT_BYTES - self.0.len() {
            return Err(std::io::Error::other("output bound"));
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn escape_bidi(text: &str) -> String {
    let mut escaped = String::new();
    for c in text.chars() {
        if matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            use std::fmt::Write;
            write!(escaped, "\\u{:04x}", c as u32).expect("string write");
        } else {
            escaped.push(c);
        }
    }
    escaped
}
/// Quoted JSON-style filenames, safe for human terminals too.
pub fn human_path(path: &str) -> String {
    let quoted = serde_json::to_string(path).expect("string serialization");
    // JSON permits DEL and C1 controls literally; human output must not.
    let mut escaped = String::new();
    for c in escape_bidi(&quoted).chars() {
        if c.is_control() {
            use std::fmt::Write;
            write!(escaped, "\\u{:04x}", c as u32).expect("string write");
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// Candidate enumeration must be bounded incrementally, before reading contents.
#[derive(Default)]
pub struct CandidatePaths(BTreeSet<String>);
impl CandidatePaths {
    pub fn insert(&mut self, path: &[u8]) -> Result<(), Code> {
        let path = validate_path(path)?;
        if !self.0.contains(path) && self.0.len() == MAX_PATHS {
            return Err(Code::LimitExceeded);
        }
        self.0.insert(path.to_owned());
        Ok(())
    }
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}
