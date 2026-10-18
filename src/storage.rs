//! Explicit owner-only file fallback. No secrets enter repository configuration.
use crate::domain::ApiOrigin;
use bootleg_core::SecretString;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialRecord {
    origin: String,
    token: SecretString,
}
pub struct CredentialStore {
    directory: PathBuf,
}
impl CredentialStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
    fn path(&self, origin: &ApiOrigin) -> PathBuf {
        // Reversible hex avoids path traversal and origin collisions.
        let key: String = origin
            .as_str()
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.directory.join(format!("{key}.json"))
    }
    /// Check storage support and directory privacy before starting authorization.
    pub fn prepare(&self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt};
            let mut b = fs::DirBuilder::new();
            b.recursive(true).mode(0o700);
            b.create(&self.directory)?;
            let m = fs::symlink_metadata(&self.directory)?;
            if !m.is_dir() || m.mode() & 0o077 != 0 {
                return Err("credential directory must be a private directory (0700)".into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err("owner-only file credentials require Unix; use origin-scoped CI credentials".into())
        }
    }
    pub fn load(
        &self,
        origin: &ApiOrigin,
    ) -> Result<Option<SecretString>, Box<dyn std::error::Error>> {
        let path = self.path(origin);
        if !path.try_exists()? {
            return Ok(None);
        }
        self.prepare()?;
        let mut f = open_private_read(&path)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut f).take(8193).read_to_end(&mut bytes)?;
        if bytes.len() > 8192 {
            return Err("invalid credential file".into());
        }
        let record: CredentialRecord =
            serde_json::from_slice(&bytes).map_err(|_| "invalid credential file")?;
        if record.origin != origin.as_str() {
            return Err("credential origin mismatch".into());
        }
        Ok(Some(record.token))
    }
    pub fn save(
        &self,
        origin: &ApiOrigin,
        token: SecretString,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.prepare()?;
        // New file + rename preserves private permissions, never follows an existing target symlink.
        let temporary = self
            .directory
            .join(format!(".login-{}", std::process::id()));
        let record = CredentialRecord {
            origin: origin.as_str().into(),
            token,
        };
        write_secret_new(&temporary, &serde_json::to_vec(&record)?)?;
        let result = fs::rename(&temporary, self.path(origin));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(())
    }
    pub fn remove(&self, origin: &ApiOrigin) -> Result<(), Box<dyn std::error::Error>> {
        match fs::remove_file(self.path(origin)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
fn open_private_read(path: &Path) -> Result<fs::File, Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let before = fs::symlink_metadata(path)?;
        if !before.is_file() || before.mode() & 0o077 != 0 {
            return Err("credential file must be a regular owner-only file (0600)".into());
        }
        let f = fs::File::open(path)?;
        let after = f.metadata()?;
        if before.ino() != after.ino() || before.dev() != after.dev() {
            return Err("credential file changed during read".into());
        }
        Ok(f)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("file credential storage unsupported".into())
    }
}
pub fn create_secret_file(path: &Path) -> Result<fs::File, Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        Ok(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("secure file delivery unsupported on this platform".into())
    }
}
pub fn write_secret_new(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        let mut f = create_secret_file(path)?;
        if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
            let _ = fs::remove_file(path);
            return Err(e.into());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, bytes);
        Err("secure file delivery unsupported on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_origin_scoped_credentials_and_no_clobber() {
        let t = tempfile::tempdir().unwrap();
        let s = CredentialStore::new(t.path().join("credentials"));
        let a = ApiOrigin::parse("https://a", false).unwrap();
        let b = ApiOrigin::parse("https://b", false).unwrap();
        s.save(&a, SecretString::new("test-secret".into())).unwrap();
        assert!(s.load(&b).unwrap().is_none());
        assert_eq!(s.load(&a).unwrap().unwrap().expose_secret(), "test-secret");
        assert!(write_secret_new(&s.path(&a), b"replacement").is_err());
        s.remove(&a).unwrap();
        assert!(s.load(&a).unwrap().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_and_public_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("token");
        write_secret_new(&p, b"secret").unwrap();
        let link = t.path().join("link");
        symlink(&p, &link).unwrap();
        assert!(open_private_read(&link).is_err());
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(open_private_read(&p).is_err());
    }
}
