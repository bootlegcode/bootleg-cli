//! Descriptor-relative source reads. Never follow any source path component.
use crate::preview::{self, Code, InputBudget};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use std::{fs::File, io::Read, os::fd::OwnedFd, path::Path, time::Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp([i128; 9]);
fn stamp(s: &fs::Stat) -> Stamp {
    Stamp([
        s.st_dev.into(),
        s.st_ino.into(),
        s.st_mode.into(),
        s.st_size.into(),
        s.st_mtime.into(),
        s.st_mtime_nsec.into(),
        s.st_ctime.into(),
        s.st_ctime_nsec.into(),
        s.st_nlink.into(),
    ])
}
fn error(e: rustix::io::Errno) -> Code {
    if matches!(e, rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) {
        Code::UnsupportedEntry
    } else {
        Code::IoFailed
    }
}
pub fn deadline(limit: Instant) -> Result<(), Code> {
    if Instant::now() >= limit {
        Err(Code::LimitExceeded)
    } else {
        Ok(())
    }
}

pub struct Root {
    fd: OwnedFd,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    parents: Vec<Stamp>,
    leaf: Option<Stamp>,
}
pub struct SourceFile {
    pub mode: preview::Mode,
    pub bytes: Vec<u8>,
}
impl Root {
    /// The root must already be canonical and validated by repository discovery.
    pub fn open(path: &Path) -> Result<Self, Code> {
        // Walk the absolute path too, so a component swap cannot redirect root.
        if !path.is_absolute() {
            return Err(Code::UnsupportedRepository);
        }
        let mut fd = fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(error)?;
        for component in path.components().skip(1) {
            let std::path::Component::Normal(name) = component else {
                return Err(Code::UnsupportedRepository);
            };
            fd = fs::openat(
                &fd,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(error)?;
        }
        Ok(Self { fd })
    }

    fn locate(&self, path: &str) -> Result<(Option<OwnedFd>, Evidence), Code> {
        preview::validate_path(path.as_bytes())?;
        let mut fd = fs::openat(
            &self.fd,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(error)?;
        let mut parents = vec![stamp(&fs::fstat(&fd).map_err(error)?)];
        let mut parts = path.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                let leaf = match fs::statat(&fd, part, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(s) => Some(stamp(&s)),
                    Err(rustix::io::Errno::NOENT) => None,
                    Err(e) => return Err(error(e)),
                };
                return Ok((Some(fd), Evidence { parents, leaf }));
            }
            fd = match fs::openat(
                &fd,
                part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => {
                    return Ok((
                        None,
                        Evidence {
                            parents,
                            leaf: None,
                        },
                    ));
                }
                Err(e) => return Err(error(e)),
            };
            parents.push(stamp(&fs::fstat(&fd).map_err(error)?));
        }
        Err(Code::UnsupportedEntry)
    }

    pub fn root_stamp(&self) -> Result<Stamp, Code> {
        fs::fstat(&self.fd).map(|s| stamp(&s)).map_err(error)
    }

    pub fn evidence(&self, path: &str) -> Result<Evidence, Code> {
        self.locate(path).map(|(_, evidence)| evidence)
    }

    pub fn read(
        &self,
        path: &str,
        budget: &mut InputBudget,
        limit: Instant,
    ) -> Result<(Option<SourceFile>, Evidence), Code> {
        deadline(limit)?;
        let (parent, before) = self.locate(path)?;
        let Some(leaf) = &before.leaf else {
            return Ok((None, before));
        };
        preview::allow_source_read(path.as_bytes(), false)?;
        let parent = parent.ok_or(Code::SourceChanged)?;
        let name = path.rsplit('/').next().ok_or(Code::UnsupportedEntry)?;
        let stat = fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(error)?;
        if stamp(&stat) != *leaf {
            return Err(Code::SourceChanged);
        }
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(Code::UnsupportedEntry);
        }
        let size = usize::try_from(stat.st_size).map_err(|_| Code::LimitExceeded)?;
        budget.reserve(size)?;
        // NONBLOCK protects against a regular file being swapped to a FIFO.
        // NOFOLLOW protects against a symlink swap between stat and open.
        let fd = fs::openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(error)?;
        if stamp(&fs::fstat(&fd).map_err(error)?) != *leaf {
            return Err(Code::SourceChanged);
        }
        let mut file = File::from(fd);
        let mut bytes = Vec::with_capacity(size);
        let mut scratch = [0; 8192];
        loop {
            deadline(limit)?;
            let count = file.read(&mut scratch).map_err(|_| Code::IoFailed)?;
            if count == 0 {
                break;
            }
            if count > size.saturating_sub(bytes.len()) {
                return Err(Code::SourceChanged);
            }
            bytes.extend_from_slice(&scratch[..count]);
        }
        if bytes.len() != size
            || stamp(&fs::fstat(&file).map_err(error)?) != *leaf
            || self.evidence(path)? != before
        {
            return Err(Code::SourceChanged);
        }
        let mode = if stat.st_mode & 0o100 == 0 {
            preview::Mode::Regular
        } else {
            preview::Mode::Executable
        };
        Ok((Some(SourceFile { mode, bytes }), before))
    }
}
