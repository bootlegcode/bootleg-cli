//! fzf process adapter. Labels are data, never shell commands or identity keys.
use crate::{
    domain::{PolicyError, terminal_text},
    selection::ChoicePrompt,
};
use std::{
    io::{IsTerminal, Write},
    process::{Command, Stdio},
};

pub struct Fzf;
fn command() -> Command {
    let mut command = Command::new("fzf");
    // Do not inherit user bindings that execute arbitrary commands on server labels,
    // change the output format, or enable multi-selection.
    for key in [
        "FZF_DEFAULT_OPTS",
        "FZF_DEFAULT_OPTS_FILE",
        "FZF_DEFAULT_COMMAND",
    ] {
        command.env_remove(key);
    }
    command
}
impl Fzf {
    pub fn preflight() -> Result<Self, PolicyError> {
        if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Err(PolicyError(
                "bootleg config requires an interactive terminal; use bootleg init --project-id for scripts",
            ));
        }
        if !command()
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return Err(PolicyError(
                "bootleg config requires fzf on PATH; install fzf and try again",
            ));
        }
        Ok(Self)
    }
}
fn encode(labels: &[String]) -> String {
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| format!("{i}\t{}\n", terminal_text(label)))
        .collect()
}
fn decode(output: &[u8], count: usize) -> Result<usize, PolicyError> {
    let value = std::str::from_utf8(output).map_err(|_| PolicyError("invalid fzf selection"))?;
    let index = value
        .split_once('\t')
        .and_then(|(index, _)| index.parse::<usize>().ok())
        .filter(|i| *i < count);
    index.ok_or(PolicyError("invalid fzf selection"))
}
impl ChoicePrompt for Fzf {
    fn choose(&mut self, prompt: &str, labels: &[String]) -> Result<usize, PolicyError> {
        let mut child = command()
            .args([
                "--no-multi",
                "--no-sort",
                "--delimiter=\t",
                "--with-nth=2..",
                "--height=40%",
                "--layout=reverse",
                "--prompt",
                &format!("{prompt}> "),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|_| PolicyError("could not start fzf"))?;
        let mut input = child
            .stdin
            .take()
            .ok_or(PolicyError("cannot send choices to fzf"))?;
        let encoded = encode(labels);
        // Feed concurrently: large organization/project lists must not deadlock pipes.
        let writer = std::thread::spawn(move || input.write_all(encoded.as_bytes()));
        let output = child
            .wait_with_output()
            .map_err(|_| PolicyError("could not read fzf selection"))?;
        let written = writer.join();
        if !output.status.success() {
            return Err(PolicyError(
                "selection cancelled; repository configuration unchanged",
            ));
        }
        if !matches!(written, Ok(Ok(()))) {
            return Err(PolicyError("could not send choices to fzf"));
        }
        decode(&output.stdout, labels.len())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires fzf installed on PATH"]
    fn real_fzf_fuzzy_matching_preserves_selection_identity() {
        let mut child = command()
            .args([
                "--filter=acm",
                "--delimiter=\t",
                "--with-nth=2..",
                "--no-multi",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("install fzf to run this adapter test");
        let labels = vec![
            "Other organization (org_other)".into(),
            "Acme organization (org_acme)".into(),
        ];
        child
            .stdin
            .take()
            .unwrap()
            .write_all(encode(&labels).as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(decode(&output.stdout, labels.len()).unwrap(), 1);
    }
    #[test]
    fn labels_cannot_inject_rows_or_terminal_controls() {
        let data = encode(&["name\n99\tbad\x1b\u{202e}".into(), "duplicate".into()]);
        assert_eq!(data.lines().count(), 2);
        assert!(!data.contains('\x1b'));
        assert!(!data.contains('\u{202e}'));
        assert_eq!(decode(b"1\tduplicate\n", 2).unwrap(), 1);
        assert!(decode(b"9\tfake", 2).is_err());
        assert!(decode(b"", 2).is_err());
    }
}
