//! Strict parsing of internal Git plumbing. No source reads or output rendering.
//! An accepted index is only one gate: repository/sparse configuration and races
//! must still be checked by capture. Unknown debug formats fail closed.
use crate::preview::{Code, MAX_PATHS, validate_path};
use crate::preview_git::ObjectId;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryMode {
    Regular,
    Executable,
    Symlink,
    Gitlink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub mode: EntryMode,
    pub object: ObjectId,
}

fn mode(value: &[u8]) -> Result<EntryMode, Code> {
    match value {
        b"100644" => Ok(EntryMode::Regular),
        b"100755" => Ok(EntryMode::Executable),
        b"120000" => Ok(EntryMode::Symlink),
        b"160000" => Ok(EntryMode::Gitlink),
        // Includes sparse-directory entries; never silently omit their contents.
        _ => Err(Code::UnsupportedRepository),
    }
}

fn object(value: &[u8], width: usize) -> Result<ObjectId, Code> {
    if !matches!(width, 40 | 64) || value.len() != width || value.iter().all(|&b| b == b'0') {
        return Err(Code::GitFailed);
    }
    ObjectId::parse(std::str::from_utf8(value).map_err(|_| Code::GitFailed)?)
        .map_err(|_| Code::GitFailed)
}

fn take<'a>(input: &mut &'a [u8], separator: u8) -> Result<&'a [u8], Code> {
    let end = input
        .iter()
        .position(|&b| b == separator)
        .ok_or(Code::GitFailed)?;
    let result = &input[..end];
    *input = &input[end + 1..];
    Ok(result)
}

fn insert(entries: &mut BTreeMap<String, Entry>, path: &[u8], entry: Entry) -> Result<(), Code> {
    let path = validate_path(path)?;
    if entries.len() == MAX_PATHS {
        return Err(Code::LimitExceeded);
    }
    if entries.insert(path.to_owned(), entry).is_some() {
        return Err(Code::GitFailed);
    }
    Ok(())
}

pub fn tree(mut input: &[u8], object_width: usize) -> Result<BTreeMap<String, Entry>, Code> {
    if !matches!(object_width, 40 | 64) {
        return Err(Code::GitFailed);
    }
    let mut entries = BTreeMap::new();
    while !input.is_empty() {
        let mut record = take(&mut input, 0)?;
        let mode = mode(take(&mut record, b' ')?)?;
        let kind = take(&mut record, b' ')?;
        if kind
            != if mode == EntryMode::Gitlink {
                b"commit".as_slice()
            } else {
                b"blob"
            }
        {
            return Err(Code::GitFailed);
        }
        let object = object(take(&mut record, b'\t')?, object_width)?;
        insert(&mut entries, record, Entry { mode, object })?;
    }
    Ok(entries)
}

fn numeric_pair(line: &[u8], prefix: &[u8], separator: &[u8]) -> Result<(), Code> {
    let line = line.strip_prefix(prefix).ok_or(Code::GitFailed)?;
    let split = line
        .windows(separator.len())
        .position(|s| s == separator)
        .ok_or(Code::GitFailed)?;
    for number in [&line[..split], &line[split + separator.len()..]] {
        if number.is_empty() || !number.iter().all(u8::is_ascii_digit) {
            return Err(Code::GitFailed);
        }
    }
    Ok(())
}

pub fn index(mut input: &[u8], object_width: usize) -> Result<BTreeMap<String, Entry>, Code> {
    if !matches!(object_width, 40 | 64) {
        return Err(Code::GitFailed);
    }
    let mut entries = BTreeMap::new();
    while !input.is_empty() {
        let mut record = take(&mut input, 0)?;
        // Lowercase means assume-unchanged; S means skip-worktree. No other
        // tags are accepted, including unmerged records.
        if take(&mut record, b' ')? != b"H" {
            return Err(Code::UnsupportedRepository);
        }
        let mode = mode(take(&mut record, b' ')?)?;
        let object = object(take(&mut record, b' ')?, object_width)?;
        if take(&mut record, b'\t')? != b"0" {
            return Err(Code::UnsupportedRepository);
        }
        let path = record;
        // --debug is deliberately treated as a closed format, not free-form
        // diagnostics. Its five lines follow the NUL-terminated pathname.
        numeric_pair(take(&mut input, b'\n')?, b"  ctime: ", b":")?;
        numeric_pair(take(&mut input, b'\n')?, b"  mtime: ", b":")?;
        numeric_pair(take(&mut input, b'\n')?, b"  dev: ", b"\tino: ")?;
        numeric_pair(take(&mut input, b'\n')?, b"  uid: ", b"\tgid: ")?;
        let mut tail = take(&mut input, b'\n')?;
        let size = take(&mut tail, b'\t')?
            .strip_prefix(b"  size: ")
            .ok_or(Code::GitFailed)?;
        if size.is_empty() || !size.iter().all(u8::is_ascii_digit) {
            return Err(Code::GitFailed);
        }
        let flags = tail.strip_prefix(b"flags: ").ok_or(Code::GitFailed)?;
        let flags = std::str::from_utf8(flags).map_err(|_| Code::GitFailed)?;
        if flags.is_empty() || !flags.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Code::GitFailed);
        }
        let flags = u32::from_str_radix(flags, 16).map_err(|_| Code::GitFailed)?;
        // Low 12 bits encode name length. Reject all remaining bits, notably
        // CE_INTENT_TO_ADD (29), CE_SKIP_WORKTREE (30), CE_VALID (15), stage,
        // extended/unknown flags. Rejecting unknown flags is intentional.
        if flags & !0xfff != 0 {
            return Err(Code::UnsupportedRepository);
        }
        insert(&mut entries, path, Entry { mode, object })?;
    }
    Ok(entries)
}

pub fn untracked(mut input: &[u8]) -> Result<BTreeSet<String>, Code> {
    let mut paths = BTreeSet::new();
    while !input.is_empty() {
        let path = validate_path(take(&mut input, 0)?)?;
        if paths.len() == MAX_PATHS {
            return Err(Code::LimitExceeded);
        }
        if !paths.insert(path.to_owned()) {
            return Err(Code::GitFailed);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_tree_and_path_validation() {
        let id = "a".repeat(40);
        let row = format!("100644 blob {id}\t-tab\tline\n雪\0");
        let parsed = tree(row.as_bytes(), 40).unwrap();
        assert_eq!(parsed["-tab\tline\n雪"].object.as_str(), id);
        assert_eq!(tree(row.as_bytes(), 64), Err(Code::GitFailed));
        assert_eq!(
            tree(format!("{row}{row}").as_bytes(), 40),
            Err(Code::GitFailed)
        );
        for row in [
            format!("100644 commit {id}\tx\0"),
            format!("100644 blob {id}\tx"),
            format!("100644 blob {}\tx\0", "0".repeat(40)),
        ] {
            assert_eq!(tree(row.as_bytes(), 40), Err(Code::GitFailed));
        }
        for path in [b"../x\0".as_slice(), b".git/config\0", b"\xff\0", b"/abs\0"] {
            assert_eq!(untracked(path), Err(Code::UnsupportedEntry));
        }
        assert_eq!(untracked(b"a\0a\0"), Err(Code::GitFailed));
        assert_eq!(untracked(b"unterminated"), Err(Code::GitFailed));
        assert_eq!(
            untracked(format!("{}\0", "x".repeat(4097)).as_bytes()),
            Err(Code::LimitExceeded)
        );
        assert!(tree(b"", 40).unwrap().is_empty());
    }
}
