use crate::domain::{ApiOrigin, PolicyError};
use bootleg_core::{OrgId, ProjectId};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryConfig {
    pub api_url: Option<String>,
    pub org_id: Option<OrgId>,
    pub project_id: Option<ProjectId>,
    // Read old generated settings, but discard agent-only configuration on save.
    #[serde(default, rename = "name_env", skip_serializing)]
    _legacy_name_env: Option<String>,
}
impl RepositoryConfig {
    /// Shared completion for explicit init and interactive config; identity only.
    pub fn initialize(origin: &ApiOrigin, org_id: Option<OrgId>, project_id: ProjectId) -> Self {
        Self {
            api_url: Some(origin.as_str().into()),
            org_id,
            project_id: Some(project_id),
            _legacy_name_env: None,
        }
    }
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "invalid nonsecret repository configuration".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("cannot read repository configuration".into()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}
pub fn resolve_origin(
    flag_or_env: Option<&str>,
    repo: &RepositoryConfig,
    development: bool,
) -> Result<ApiOrigin, PolicyError> {
    ApiOrigin::parse(
        flag_or_env.or(repo.api_url.as_deref()).ok_or(PolicyError(
            "set --api-url, BOOTLEG_API_URL, or repository api_url",
        ))?,
        development,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn init_and_interactive_config_share_identity_only_completion() {
        let origin = ApiOrigin::parse("https://example.test", false).unwrap();
        let config = RepositoryConfig::initialize(&origin, None, "42".parse().unwrap());
        let output = serde_json::to_value(&config).unwrap();
        assert_eq!(output["project_id"], "42");
        assert_eq!(output["api_url"], "https://example.test/");
        assert!(output.get("name_env").is_none());
        let old: RepositoryConfig =
            serde_json::from_str(r#"{"name_env":"POD_NAME","project_id":"42"}"#).unwrap();
        assert!(serde_json::to_value(old).unwrap().get("name_env").is_none());
    }
    #[test]
    fn precedence_and_no_secret_fields() {
        let c = RepositoryConfig {
            api_url: Some("https://repo.test".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_origin(Some("https://flag.test"), &c, false)
                .unwrap()
                .as_str(),
            "https://flag.test/"
        );
        assert_eq!(
            resolve_origin(None, &c, false).unwrap().as_str(),
            "https://repo.test/"
        );
        assert!(serde_json::from_str::<RepositoryConfig>(r#"{"project_token":"secret"}"#).is_err());
    }
}
