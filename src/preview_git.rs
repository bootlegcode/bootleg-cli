//! Restricted, bounded Git plumbing for preview. Not a source-capture adapter.
//! Unix sockets keep output reads nonblocking without reader threads or temporary files.
#[cfg(unix)]
mod unix {
    use std::io::{ErrorKind, Read};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
    const MAX_OUTPUT: usize = 4096;
    use crate::preview::{MAX_FILE_BYTES, MAX_PATH_BYTES, MAX_PATHS};

    // Mode/type/object-ID/stage/tag separators, in addition to the path itself.
    const MAX_RECORD_BYTES: usize = MAX_PATH_BYTES + 96;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum GitError {
        InvalidInput,
        Failed,
        Deadline,
        Limit,
    }

    /// Only full object IDs are accepted; callers cannot inject revisions or options.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ObjectId(String);

    impl ObjectId {
        pub fn as_str(&self) -> &str {
            &self.0
        }

        pub fn parse(value: &str) -> Result<Self, GitError> {
            if !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(GitError::InvalidInput);
            }
            Ok(Self(value.to_ascii_lowercase()))
        }
    }

    pub enum HistoryCommand<'a> {
        Head,
        Root,
        GitDirectory,
        SafetyConfig,
        Attributes(&'a str),
        Ignored(&'a str),
        ObjectFormat,
        CommitExists(&'a ObjectId),
        IsAncestor(&'a ObjectId, &'a ObjectId),
        /// Recursive leaf entries; gitlinks are listed, never traversed.
        Tree(&'a ObjectId),
        /// Stage, -v tags and debug flags; requires strict typed parsing.
        Index,
        /// Standard ignore rules apply only to this untracked enumeration.
        Untracked,
        BlobSize(&'a ObjectId),
        /// Raw object bytes, without filters or textconv. Caller reserves the
        /// cumulative input budget and checks sensitive paths before requesting.
        Blob(&'a ObjectId),
        /// The capture adapter has already reserved this exact object size.
        BlobWithin(&'a ObjectId, usize),
    }

    impl HistoryCommand<'_> {
        fn records(&self) -> bool {
            matches!(self, Self::Tree(_) | Self::Index | Self::Untracked)
        }

        fn output_limit(&self) -> usize {
            if self.records() {
                MAX_PATHS * (MAX_RECORD_BYTES + 1 + 512)
            } else if matches!(self, Self::Attributes(_)) {
                3 * (MAX_PATH_BYTES + 128)
            } else if let Self::BlobWithin(_, size) = self {
                (*size).min(MAX_FILE_BYTES)
            } else if matches!(self, Self::Blob(_)) {
                MAX_FILE_BYTES
            } else {
                MAX_OUTPUT
            }
        }
    }

    #[derive(Default)]
    struct RecordBudget {
        count: usize,
        bytes: usize,
        detail_lines: usize,
        detail_bytes: usize,
        index: bool,
    }

    impl RecordBudget {
        fn accept(&mut self, bytes: &[u8]) -> Result<(), GitError> {
            for &byte in bytes {
                if self.detail_lines != 0 {
                    self.detail_bytes += 1;
                    if self.detail_bytes > 512 {
                        return Err(GitError::Limit);
                    }
                    if byte == b'\n' {
                        self.detail_lines -= 1;
                    }
                    continue;
                }
                if self.count == MAX_PATHS {
                    return Err(GitError::Limit);
                }
                if byte == 0 {
                    if self.bytes == 0 {
                        return Err(GitError::Failed);
                    }
                    self.count += 1;
                    self.bytes = 0;
                    if self.index {
                        self.detail_lines = 5;
                        self.detail_bytes = 0;
                    }
                } else {
                    self.bytes += 1;
                    if self.bytes > MAX_RECORD_BYTES {
                        return Err(GitError::Limit);
                    }
                }
            }
            Ok(())
        }
    }

    #[derive(Debug)]
    pub struct GitOutput {
        /// Exit-code 1 is accepted only for ancestry, ignore and config queries.
        pub affirmative: bool,
        pub bytes: Vec<u8>,
    }

    pub struct HistoryGit {
        executable: PathBuf,
    }

    struct Running(Child);
    impl Drop for Running {
        fn drop(&mut self) {
            // Also reap on every timeout, output-limit and read-error path.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    impl HistoryGit {
        /// The executable is trusted installation configuration, never repository input.
        pub fn new(executable: PathBuf) -> Result<Self, GitError> {
            if !executable.is_absolute() {
                return Err(GitError::InvalidInput);
            }
            Ok(Self { executable })
        }

        /// Source enumeration requires a previously validated worktree root as
        /// `directory`; this runner does not discover or validate repositories.
        /// Returned bytes are internal evidence, never user-facing diagnostics.
        pub fn run(
            &self,
            directory: &Path,
            operation: HistoryCommand<'_>,
            invocation_deadline: Instant,
        ) -> Result<GitOutput, GitError> {
            let limit = operation.output_limit();
            self.run_bounded(directory, operation, invocation_deadline, limit)
        }

        fn run_bounded(
            &self,
            directory: &Path,
            operation: HistoryCommand<'_>,
            invocation_deadline: Instant,
            output_limit: usize,
        ) -> Result<GitOutput, GitError> {
            let deadline = invocation_deadline.min(Instant::now() + OPERATION_TIMEOUT);
            if Instant::now() >= deadline {
                return Err(GitError::Deadline);
            }
            let (mut reader, writer) = UnixStream::pair().map_err(|_| GitError::Failed)?;
            reader.set_nonblocking(true).map_err(|_| GitError::Failed)?;
            let mut command = Command::new(&self.executable);
            command.env_clear();
            // Preserve standard user configuration (not arbitrary GIT_* process overrides).
            for name in ["HOME", "XDG_CONFIG_HOME"] {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            command
                .env("PATH", "/usr/bin:/bin")
                .env("LC_ALL", "C")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env("GIT_NO_LAZY_FETCH", "1")
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_PAGER", "")
                .env("GIT_ATTR_NOSYSTEM", "1")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .current_dir(directory)
                .stdin(Stdio::null())
                .stdout(Stdio::from(OwnedFd::from(writer)))
                .stderr(Stdio::null())
                .args([
                    "--no-pager",
                    "--no-optional-locks",
                    "--literal-pathspecs",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "core.untrackedCache=false",
                    "-c",
                    "core.preloadIndex=false",
                    "-c",
                    "protocol.allow=never",
                    "-c",
                    "credential.helper=",
                ]);
            if let HistoryCommand::Attributes(path) | HistoryCommand::Ignored(path) = &operation {
                crate::preview::validate_path(path.as_bytes())
                    .map_err(|_| GitError::InvalidInput)?;
            }
            let config = matches!(
                operation,
                HistoryCommand::SafetyConfig | HistoryCommand::Ignored(_)
            );
            let records = operation.records();
            let mut record_budget = RecordBudget {
                index: matches!(operation, HistoryCommand::Index),
                ..RecordBudget::default()
            };
            let ancestry = matches!(operation, HistoryCommand::IsAncestor(_, _));
            let require_commit = matches!(operation, HistoryCommand::CommitExists(_));
            match operation {
                HistoryCommand::Root => {
                    command.args(["rev-parse", "--show-toplevel"]);
                }
                HistoryCommand::GitDirectory => {
                    command.args(["rev-parse", "--absolute-git-dir"]);
                }
                HistoryCommand::SafetyConfig => {
                    command.args(["config", "--null", "--get-regexp", "^(core\\.(bare|sparsecheckout|sparsecheckoutcone)|extensions\\.(worktreeconfig|partialclone)|remote\\..*\\.promisor)$"]);
                }
                HistoryCommand::Ignored(path) => {
                    // check-ignore accepts literal pathnames but rejects the
                    // global literal-pathspec magic used by other plumbing.
                    // Prefix ./ so a pathname beginning with ':' is not magic.
                    command.args([
                        "--no-literal-pathspecs",
                        "check-ignore",
                        "--quiet",
                        "--no-index",
                        "--",
                        &format!("./{path}"),
                    ]);
                }
                HistoryCommand::Attributes(path) => {
                    command.args([
                        "check-attr",
                        "-z",
                        "filter",
                        "working-tree-encoding",
                        "diff",
                        "--",
                        path,
                    ]);
                }
                HistoryCommand::Head => {
                    command.args(["rev-parse", "--verify", "HEAD^{commit}"]);
                }
                HistoryCommand::ObjectFormat => {
                    command.args(["rev-parse", "--show-object-format"]);
                }
                HistoryCommand::CommitExists(id) => {
                    command.args(["cat-file", "-t", &id.0]);
                }
                HistoryCommand::IsAncestor(base, head) => {
                    command.args(["merge-base", "--is-ancestor", &base.0, &head.0]);
                }
                HistoryCommand::Tree(id) => {
                    command.args(["ls-tree", "-r", "-z", "--full-tree", &id.0]);
                }
                HistoryCommand::Index => {
                    command.args(["ls-files", "--full-name", "--stage", "-v", "--debug", "-z"]);
                }
                HistoryCommand::Untracked => {
                    command.args([
                        "ls-files",
                        "--full-name",
                        "--others",
                        "--exclude-standard",
                        "-z",
                    ]);
                }
                HistoryCommand::BlobSize(id) => {
                    command.args(["cat-file", "-s", &id.0]);
                }
                HistoryCommand::Blob(id) | HistoryCommand::BlobWithin(id, _) => {
                    command.args(["cat-file", "blob", &id.0]);
                }
            }
            let mut child = Running(command.spawn().map_err(|_| GitError::Failed)?);
            // Command owns the parent's copy of stdout until dropped.
            drop(command);
            let mut output = Vec::new();
            let mut scratch = [0; 1024];
            let mut eof = false;
            loop {
                if Instant::now() >= deadline {
                    return Err(GitError::Deadline);
                }
                if !eof {
                    match reader.read(&mut scratch) {
                        Ok(0) => eof = true,
                        Ok(count) => {
                            if count > output_limit.saturating_sub(output.len()) {
                                return Err(GitError::Limit);
                            }
                            if records {
                                record_budget.accept(&scratch[..count])?;
                            }
                            output.extend_from_slice(&scratch[..count]);
                            continue;
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                        Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => return Err(GitError::Failed),
                    }
                }
                if let Some(status) = child.0.try_wait().map_err(|_| GitError::Failed)?
                    && eof
                {
                    let affirmative = status.success();
                    if (!affirmative && !((ancestry || config) && status.code() == Some(1)))
                        || (require_commit && output != b"commit\n")
                        || (records
                            && (record_budget.bytes != 0 || record_budget.detail_lines != 0))
                    {
                        return Err(GitError::Failed);
                    }
                    return Ok(GitOutput {
                        affirmative,
                        bytes: output,
                    });
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn git(dir: &Path, args: &[&str]) -> String {
            let output = Command::new("/usr/bin/git")
                .current_dir(dir)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
            fn visit(
                root: &Path,
                path: &Path,
                result: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>,
            ) {
                for entry in std::fs::read_dir(path).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    if entry.file_type().unwrap().is_dir() {
                        result.insert(path.strip_prefix(root).unwrap().to_owned(), None);
                        visit(root, &path, result);
                    } else {
                        result.insert(
                            path.strip_prefix(root).unwrap().to_owned(),
                            Some(std::fs::read(path).unwrap()),
                        );
                    }
                }
            }
            let mut result = std::collections::BTreeMap::new();
            visit(root, root, &mut result);
            result
        }

        #[test]
        fn actual_history_is_bounded_and_read_only() {
            let dir = tempfile::tempdir().unwrap();
            git(dir.path(), &["init", "-q"]);
            git(
                dir.path(),
                &["config", "user.email", "test@example.invalid"],
            );
            git(dir.path(), &["config", "user.name", "Test"]);
            git(dir.path(), &["commit", "--allow-empty", "-qm", "first"]);
            let first = ObjectId::parse(&git(dir.path(), &["rev-parse", "HEAD"])).unwrap();
            git(dir.path(), &["commit", "--allow-empty", "-qm", "second"]);
            let second = ObjectId::parse(&git(dir.path(), &["rev-parse", "HEAD"])).unwrap();
            git(
                dir.path(),
                &["config", "core.fsmonitor", "touch helper-ran"],
            );
            git(dir.path(), &["config", "core.pager", "touch helper-ran"]);
            let before = snapshot(dir.path());
            let runner = HistoryGit::new("/usr/bin/git".into()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            assert!(
                runner
                    .run(
                        dir.path(),
                        HistoryCommand::IsAncestor(&first, &second),
                        deadline
                    )
                    .unwrap()
                    .affirmative
            );
            assert!(
                !runner
                    .run(
                        dir.path(),
                        HistoryCommand::IsAncestor(&second, &first),
                        deadline
                    )
                    .unwrap()
                    .affirmative
            );
            assert_eq!(
                runner
                    .run(dir.path(), HistoryCommand::Head, deadline)
                    .unwrap()
                    .bytes,
                format!("{}\n", second.0).as_bytes()
            );
            assert_eq!(
                runner
                    .run(dir.path(), HistoryCommand::ObjectFormat, deadline)
                    .unwrap()
                    .bytes,
                b"sha1\n"
            );
            runner
                .run(dir.path(), HistoryCommand::CommitExists(&first), deadline)
                .unwrap();
            let missing = ObjectId::parse(&"0".repeat(40)).unwrap();
            assert_eq!(
                runner
                    .run(dir.path(), HistoryCommand::CommitExists(&missing), deadline)
                    .unwrap_err(),
                GitError::Failed
            );
            assert_eq!(
                runner
                    .run_bounded(dir.path(), HistoryCommand::Head, deadline, 1)
                    .unwrap_err(),
                GitError::Limit
            );
            assert_eq!(
                runner
                    .run(dir.path(), HistoryCommand::Head, Instant::now())
                    .unwrap_err(),
                GitError::Deadline
            );
            assert_eq!(before, snapshot(dir.path()));
            assert!(!dir.path().join(".git/index.lock").exists());
            assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), second.0);
        }

        #[test]
        fn blocked_and_streaming_children_are_bounded_and_reaped() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let executable = dir.path().join("process-fixture");
            // Process fixtures only: no HTTP or Git repository semantics are mocked.
            for (script, expected) in [
                ("#!/bin/sh\nexec /bin/sleep 10\n", GitError::Deadline),
                (
                    "#!/bin/sh\nwhile :; do printf '0123456789'; done\n",
                    GitError::Limit,
                ),
                (
                    "#!/bin/sh\nprintf 'secret diagnostic' >&2\nexit 7\n",
                    GitError::Failed,
                ),
            ] {
                std::fs::write(&executable, script).unwrap();
                std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
                    .unwrap();
                let runner = HistoryGit::new(executable.clone()).unwrap();
                let start = Instant::now();
                assert_eq!(
                    runner
                        .run(
                            dir.path(),
                            HistoryCommand::Head,
                            start + Duration::from_millis(200)
                        )
                        .unwrap_err(),
                    expected
                );
                assert!(start.elapsed() < Duration::from_secs(2));
            }
        }

        #[test]
        fn actual_source_plumbing_is_raw_bounded_and_read_only() {
            let dir = tempfile::tempdir().unwrap();
            git(dir.path(), &["init", "-q"]);
            git(
                dir.path(),
                &["config", "user.email", "test@example.invalid"],
            );
            git(dir.path(), &["config", "user.name", "Test"]);
            let unusual = "-tab\tline\n雪.txt";
            let raw = b"raw\r\ntext without final newline";
            std::fs::write(dir.path().join(unusual), raw).unwrap();
            std::fs::write(dir.path().join(".gitignore"), "ignored\n").unwrap();
            git(dir.path(), &["add", "--", "."]);
            git(dir.path(), &["commit", "-qm", "source"]);
            let head = ObjectId::parse(&git(dir.path(), &["rev-parse", "HEAD"])).unwrap();
            let blob =
                ObjectId::parse(&git(dir.path(), &["rev-parse", &format!("HEAD:{unusual}")]))
                    .unwrap();
            // Configure helpers only after fixture creation. None may run during reads.
            for (key, value) in [
                ("core.fsmonitor", "touch helper-ran"),
                ("diff.external", "touch helper-ran"),
                ("diff.sentinel.textconv", "touch helper-ran"),
                ("filter.sentinel.clean", "touch helper-ran"),
                ("filter.sentinel.smudge", "touch helper-ran"),
            ] {
                git(dir.path(), &["config", key, value]);
            }
            std::fs::write(
                dir.path().join(".gitattributes"),
                "*.txt filter=sentinel diff=sentinel\n",
            )
            .unwrap();
            std::fs::write(dir.path().join(unusual), "worktree differs").unwrap();
            std::fs::write(dir.path().join("untracked\nname"), "new").unwrap();
            std::fs::write(dir.path().join("ignored"), "excluded").unwrap();
            let before = snapshot(dir.path());
            let runner = HistoryGit::new("/usr/bin/git".into()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let tree = runner
                .run(dir.path(), HistoryCommand::Tree(&head), deadline)
                .unwrap()
                .bytes;
            assert!(
                tree.split(|&b| b == 0)
                    .any(|r| r.ends_with(format!("\t{unusual}").as_bytes()))
            );
            let index = runner
                .run(dir.path(), HistoryCommand::Index, deadline)
                .unwrap()
                .bytes;
            assert!(
                index.split(|&b| b == 0).any(|r| r.starts_with(b"H 100644 ")
                    && r.ends_with(format!(" 0\t{unusual}").as_bytes()))
            );
            let untracked = runner
                .run(dir.path(), HistoryCommand::Untracked, deadline)
                .unwrap()
                .bytes;
            assert_eq!(untracked, b".gitattributes\0untracked\nname\0");
            assert_eq!(
                runner
                    .run(dir.path(), HistoryCommand::BlobSize(&blob), deadline)
                    .unwrap()
                    .bytes,
                format!("{}\n", raw.len()).as_bytes()
            );
            assert_eq!(
                runner
                    .run(dir.path(), HistoryCommand::Blob(&blob), deadline)
                    .unwrap()
                    .bytes,
                raw
            );
            assert_eq!(
                runner
                    .run_bounded(dir.path(), HistoryCommand::Tree(&head), deadline, 1)
                    .unwrap_err(),
                GitError::Limit
            );
            assert_eq!(before, snapshot(dir.path()));
            assert!(!dir.path().join("helper-ran").exists());
        }

        #[test]
        fn actual_blob_reads_enforce_size_and_preserve_binary_bytes() {
            let dir = tempfile::tempdir().unwrap();
            git(dir.path(), &["init", "-q"]);
            let path = dir.path().join("binary");
            let runner = HistoryGit::new("/usr/bin/git".into()).unwrap();
            for size in [MAX_FILE_BYTES, MAX_FILE_BYTES + 1] {
                std::fs::write(&path, vec![0; size]).unwrap();
                let id =
                    ObjectId::parse(&git(dir.path(), &["hash-object", "-w", "binary"])).unwrap();
                let before = snapshot(dir.path());
                let result = runner.run(
                    dir.path(),
                    HistoryCommand::Blob(&id),
                    Instant::now() + Duration::from_secs(10),
                );
                if size == MAX_FILE_BYTES {
                    assert_eq!(result.unwrap().bytes, vec![0; size]);
                } else {
                    assert_eq!(result.unwrap_err(), GitError::Limit);
                }
                assert_eq!(before, snapshot(dir.path()));
            }
        }

        #[test]
        fn record_budgets_are_incremental_and_fail_before_retention() {
            let mut budget = RecordBudget::default();
            budget.accept(&vec![b'x'; MAX_RECORD_BYTES]).unwrap();
            assert_eq!(budget.accept(b"x"), Err(GitError::Limit));
            let mut budget = RecordBudget::default();
            for _ in 0..MAX_PATHS {
                budget.accept(b"x").unwrap();
                budget.accept(b"\0").unwrap();
            }
            budget.accept(b"").unwrap();
            assert_eq!(budget.accept(b"x"), Err(GitError::Limit));
            assert_eq!(RecordBudget::default().accept(b"\0"), Err(GitError::Failed));
        }

        #[test]
        fn index_record_budget_bounds_and_frames_debug_details() {
            let mut budget = RecordBudget {
                index: true,
                ..RecordBudget::default()
            };
            // Filenames can contain newlines; only the NUL starts debug lines.
            for part in [
                b"H header\tline\nname".as_slice(),
                b"\0one\ntwo\n",
                b"three\nfour\nfive\n",
            ] {
                budget.accept(part).unwrap();
            }
            assert_eq!(budget.count, 1);
            assert_eq!(budget.bytes, 0);
            assert_eq!(budget.detail_lines, 0);
            budget.accept(b"next\0").unwrap();
            budget.accept(&[b'x'; 512]).unwrap();
            assert_eq!(budget.accept(b"\n"), Err(GitError::Limit));
        }

        #[test]
        fn rejects_revision_and_executable_injection() {
            for value in ["HEAD", "--help", "abc", &"g".repeat(40), &"a".repeat(41)] {
                assert!(ObjectId::parse(value).is_err());
            }
            assert!(HistoryGit::new("git".into()).is_err());
        }
    }
}

#[cfg(unix)]
pub use unix::*;
