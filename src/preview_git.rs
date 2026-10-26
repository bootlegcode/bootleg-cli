//! Restricted, bounded Git history plumbing for preview. Not a source-capture adapter.
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum GitError {
        InvalidInput,
        Failed,
        Deadline,
        Limit,
    }

    /// Only full object IDs are accepted; callers cannot inject revisions or options.
    #[derive(Debug, Clone)]
    pub struct ObjectId(String);

    impl ObjectId {
        pub fn parse(value: &str) -> Result<Self, GitError> {
            if !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(GitError::InvalidInput);
            }
            Ok(Self(value.to_ascii_lowercase()))
        }
    }

    pub enum HistoryCommand<'a> {
        Head,
        ObjectFormat,
        CommitExists(&'a ObjectId),
        IsAncestor(&'a ObjectId, &'a ObjectId),
    }

    #[derive(Debug)]
    pub struct GitOutput {
        /// False is meaningful only for IsAncestor (Git exit code 1).
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

        pub fn run(
            &self,
            directory: &Path,
            operation: HistoryCommand<'_>,
            invocation_deadline: Instant,
        ) -> Result<GitOutput, GitError> {
            self.run_bounded(directory, operation, invocation_deadline, MAX_OUTPUT)
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
            let ancestry = matches!(operation, HistoryCommand::IsAncestor(_, _));
            let require_commit = matches!(operation, HistoryCommand::CommitExists(_));
            match operation {
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
                    if (!affirmative && !(ancestry && status.code() == Some(1)))
                        || (require_commit && output != b"commit\n")
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
