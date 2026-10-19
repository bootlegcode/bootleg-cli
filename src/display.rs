//! Human-readable discovery output. JSON continues to use the API DTOs unchanged.
pub mod style;
use crate::domain::terminal_text;
use bootleg_core::{OrgId, ProjectId, api::*};
use style::{Renderer, Tone};

pub fn identity(identity: &IdentityResponse) -> String {
    identity_with_color(identity, false)
}

pub fn identity_with_color(identity: &IdentityResponse, color: bool) -> String {
    let expires = &identity.expires_at;
    let day = expires.format("%-d").to_string();
    let suffix = ordinal_suffix(&day);
    let rows = [
        ("Email", terminal_text(&identity.email)),
        ("User ID", identity.user_id.to_string()),
        (
            "Session Expires",
            format!(
                "{} {day}{suffix}, {} UTC",
                expires.format("%B"),
                expires.format("%Y %H:%M:%S")
            ),
        ),
    ];
    Renderer {
        color,
        ..Renderer::stdout()
    }
    .panel(
        "Signed-in user",
        &rows
            .iter()
            .map(|(label, value)| format!("{label}: {value}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Tone::Info,
    )
}

fn ordinal_suffix(day: &str) -> &'static str {
    match day {
        "1" | "21" | "31" => "st",
        "2" | "22" => "nd",
        "3" | "23" => "rd",
        _ => "th",
    }
}

pub fn organizations(items: &[Organization], current: Option<&OrgId>) -> String {
    if items.is_empty() {
        return Renderer::stdout().panel("Organizations", "No organizations found.", Tone::Info);
    }
    let mut rows = Vec::new();
    for item in items {
        let role = match item.role {
            OrgRole::Owner => "owner",
            OrgRole::Admin => "admin",
            OrgRole::Member => "member",
        };
        rows.push(vec![
            item.org_id.to_string(),
            terminal_text(item.name.as_str()),
            role.into(),
        ]);
    }
    Renderer::stdout().table_with_selection(
        "Organizations",
        &["ID", "Name", "Role"],
        rows,
        items.iter().position(|item| current == Some(&item.org_id)),
    )
}

pub fn projects(items: &[Project], current: Option<ProjectId>) -> String {
    if items.is_empty() {
        return Renderer::stdout().panel("Projects", "No projects found.", Tone::Info);
    }
    let mut rows = Vec::new();
    for item in items {
        let status = if item.active { "active" } else { "inactive" };
        rows.push(vec![
            item.project_id.to_string(),
            terminal_text(item.name.as_str()),
            status.into(),
        ]);
    }
    Renderer::stdout().table_with_selection(
        "Projects",
        &["ID", "Name", "Status"],
        rows,
        items
            .iter()
            .position(|item| current.as_ref() == Some(&item.project_id)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_output_marks_selection_and_handles_empty_results() {
        style::configure(true);
        let response: ProjectTokenResponse = serde_json::from_str(include_str!(
            "../../bootleg-core/fixtures/v1/project-token.json"
        ))
        .unwrap();
        let mut project = response.project;
        let org = Organization {
            org_id: project.org_id.clone(),
            name: project.name.clone(),
            role: OrgRole::Member,
        };
        assert!(
            organizations(std::slice::from_ref(&org), Some(&org.org_id)).contains(&format!(
                "Current: {} ({})",
                org.name.as_str(),
                org.org_id
            ))
        );
        assert!(organizations(&[org], None).contains("member"));
        project.active = false;
        let text = projects(
            std::slice::from_ref(&project),
            Some(project.project_id.clone()),
        );
        assert!(text.contains(&format!(
            "Current: {} ({})",
            project.name.as_str(),
            project.project_id
        )));
        assert!(text.contains("inactive"));
        assert!(!text.contains(response.project_token.expose_secret()));
        assert!(projects(&[], None).contains("No projects found."));
        assert!(organizations(&[], None).contains("No organizations found."));
    }

    #[test]
    fn expiration_uses_readable_date_and_ordinal_suffixes() {
        for day in 1..=31 {
            let identity: IdentityResponse = serde_json::from_value(serde_json::json!({
                "user_id": bootleg_core::UserId::new(),
                "email": "user@example.test",
                "cli_credential_id": bootleg_core::CliCredentialId::new(),
                "expires_at": format!("2026-10-{day:02}T14:30:05Z")
            }))
            .unwrap();
            let suffix = match day {
                1 | 21 | 31 => "st",
                2 | 22 => "nd",
                3 | 23 => "rd",
                _ => "th",
            };
            assert!(
                super::identity(&identity)
                    .contains(&format!("October {day}{suffix}, 2026 14:30:05 UTC"))
            );
        }
    }

    #[test]
    fn identity_sanitizes_server_text() {
        let identity: IdentityResponse = serde_json::from_value(serde_json::json!({
            "user_id": bootleg_core::UserId::new(),
            "email": "user\u{001b}\u{202e}@example.test",
            "cli_credential_id": bootleg_core::CliCredentialId::new(),
            "expires_at": "2030-01-01T00:00:00Z"
        }))
        .unwrap();
        let text = super::identity(&identity);
        assert!(text.contains("Email: user@example.test"));
        assert!(text.contains("January 1st, 2030 00:00:00 UTC"));
        assert!(!text.contains('\u{001b}'));
        let colored = identity_with_color(&identity, true);
        assert!(colored.contains('\x1b'));
        assert!(colored.contains("Email: user@example.test"));
        assert!(!text.contains("Credential:"));
        assert!(!text.contains(&identity.cli_credential_id.to_string()));
    }
}
