//! Use cases and safe output projections. No network-shaped test doubles.
use crate::domain::PolicyError;
use bootleg_core::{
    SecretString,
    api::{Project, ProjectTokenResponse},
};

/// Delivery is a business port: issuing a key must never accidentally print it.
pub trait ProjectCredentialDelivery {
    fn deliver(&mut self, credential: &SecretString) -> Result<(), PolicyError>;
}
pub fn complete_project_issuance(
    response: ProjectTokenResponse,
    delivery: &mut impl ProjectCredentialDelivery,
) -> Result<Project, PolicyError> {
    delivery.deliver(&response.project_token)?;
    Ok(response.project)
}
pub fn require_rotation_confirmation(confirmed: bool) -> Result<(), PolicyError> {
    if confirmed {
        Ok(())
    } else {
        Err(PolicyError(
            "rotation invalidates all old agent keys; pass --confirm to proceed",
        ))
    }
}

/// Bootstrap is optional diagnostic input, never a gate or command result.
/// Returned text is exclusively for the stderr adapter.
pub fn notice_diagnostics(
    bootstrap: Option<&bootleg_core::communication::BootstrapResponse>,
    mode: crate::domain::NoticeMode,
    json: bool,
    tty: bool,
) -> Vec<String> {
    if !mode.show(json, tty) {
        return Vec::new();
    }
    bootstrap
        .into_iter()
        .flat_map(|b| b.notices.as_slice())
        .map(|n| {
            format!(
                "{:?}: {}",
                n.level,
                crate::domain::terminal_text(n.message.as_str())
            )
        })
        .collect()
}

/// Selection is a workflow decision over already-authorized discovery results.
/// It never recovers a project credential or treats a public identity as one.
pub fn select_project<'a>(
    projects: &'a [Project],
    org: &bootleg_core::OrgId,
    id: bootleg_core::ProjectId,
) -> Result<&'a Project, PolicyError> {
    projects
        .iter()
        .find(|p| p.org_id == *org && p.project_id == id && p.active)
        .ok_or(PolicyError("project not eligible in selected organization"))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Vault {
        secret: Option<String>,
        fail: bool,
    }
    impl ProjectCredentialDelivery for Vault {
        fn deliver(&mut self, key: &SecretString) -> Result<(), PolicyError> {
            if self.fail {
                return Err(PolicyError("delivery failed"));
            }
            self.secret = Some(key.expose_secret().into());
            Ok(())
        }
    }
    #[test]
    fn selection_rejects_ineligible_and_never_returns_credentials() {
        let response: ProjectTokenResponse = serde_json::from_str(include_str!(
            "../../bootleg-core/fixtures/v1/project-token.json"
        ))
        .unwrap();
        let mut projects = vec![response.project];
        let org = projects[0].org_id.clone();
        let id = projects[0].project_id;
        assert!(select_project(&projects, &org, id).is_ok());
        assert!(select_project(&projects, &org, "99999".parse().unwrap()).is_err());
        assert!(select_project(&projects, &bootleg_core::OrgId::new(), id).is_err());
        assert!(select_project(&[], &org, id).is_err());
        projects[0].active = false;
        assert!(select_project(&projects, &org, id).is_err());
    }
    #[test]
    fn bootstrap_failure_and_error_level_banner_do_not_gate_commands() {
        use crate::domain::NoticeMode::*;
        assert!(notice_diagnostics(None, Always, false, true).is_empty());
        let b = serde_json::from_str(r#"{"protocol_version":1,"notices":[{"id":"n1","level":"error","message":"An announcement, not a command failure."}]}"#).unwrap();
        assert!(notice_diagnostics(Some(&b), Never, false, true).is_empty());
        assert!(notice_diagnostics(Some(&b), Auto, true, true).is_empty());
        assert!(notice_diagnostics(Some(&b), Auto, false, false).is_empty());
        let messages = notice_diagnostics(Some(&b), Always, true, false);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("An announcement, not a command failure."));
    }
    #[test]
    fn real_contract_fixture_secret_delivery_and_safe_projection() {
        let response: ProjectTokenResponse = serde_json::from_str(include_str!(
            "../../bootleg-core/fixtures/v1/project-token.json"
        ))
        .unwrap();
        let key = response.project_token.expose_secret().to_string();
        let mut vault = Vault {
            secret: None,
            fail: false,
        };
        let project = complete_project_issuance(response.clone(), &mut vault).unwrap();
        assert_eq!(vault.secret.as_deref(), Some(key.as_str()));
        let output = serde_json::to_string(&project).unwrap();
        assert!(!output.contains(&key));
        assert!(!output.contains("project_token"));
        assert!(!output.contains("secret_hash"));
        vault.fail = true;
        assert!(complete_project_issuance(response, &mut vault).is_err());
        assert!(require_rotation_confirmation(false).is_err());
        assert!(require_rotation_confirmation(true).is_ok());
    }
}
