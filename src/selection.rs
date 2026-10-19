//! Configuration selection use cases. Prompts are a business port, not API mocks.
use crate::domain::PolicyError;
use bootleg_core::{OrgId, ProjectId};

pub struct OrganizationChoice {
    pub id: OrgId,
    pub name: String,
}
pub struct ProjectChoice {
    pub id: ProjectId,
    pub org_id: OrgId,
    pub name: String,
    pub active: bool,
}
/// Return an index in the provided choices, or an error on cancellation.
pub trait ChoicePrompt {
    fn choose(&mut self, prompt: &str, labels: &[String]) -> Result<usize, PolicyError>;
}
pub fn organization(
    choices: &[OrganizationChoice],
    prompt: &mut impl ChoicePrompt,
) -> Result<OrgId, PolicyError> {
    let index = match choices.len() {
        0 => {
            return Err(PolicyError(
                "you do not belong to any eligible organizations",
            ));
        }
        1 => 0,
        _ => prompt.choose(
            "Organization",
            &choices
                .iter()
                .map(|o| format!("{} ({})", o.name, o.id))
                .collect::<Vec<_>>(),
        )?,
    };
    choices
        .get(index)
        .map(|o| o.id.clone())
        .ok_or(PolicyError("invalid organization selection"))
}
pub fn project(
    org: &OrgId,
    choices: &[ProjectChoice],
    prompt: &mut impl ChoicePrompt,
) -> Result<ProjectId, PolicyError> {
    let eligible: Vec<_> = choices
        .iter()
        .filter(|p| p.org_id == *org && p.active)
        .collect();
    if eligible.is_empty() {
        return Err(PolicyError(
            "this organization has no active projects; create one with bootleg projects create",
        ));
    }
    let index = prompt.choose(
        "Project",
        &eligible
            .iter()
            .map(|p| format!("{} ({})", p.name, p.id))
            .collect::<Vec<_>>(),
    )?;
    eligible
        .get(index)
        .map(|p| p.id.clone())
        .ok_or(PolicyError("invalid project selection"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bootleg_core::api::{Organization, Page};
    struct Prompt {
        calls: usize,
        selected: Result<usize, &'static str>,
        labels: Vec<String>,
    }
    impl ChoicePrompt for Prompt {
        fn choose(&mut self, _: &str, labels: &[String]) -> Result<usize, PolicyError> {
            self.calls += 1;
            self.labels = labels.to_vec();
            self.selected.map_err(PolicyError)
        }
    }
    fn org() -> OrgId {
        let fixture: Page<Organization> =
            serde_json::from_str(include_str!("../../bootleg-core/fixtures/v1/orgs.json")).unwrap();
        fixture.items[0].org_id.clone()
    }
    #[test]
    fn sole_org_is_automatic_multiple_orgs_prompt_and_empty_fails() {
        let id = org();
        let mut prompt = Prompt {
            calls: 0,
            selected: Ok(1),
            labels: vec![],
        };
        assert!(organization(&[], &mut prompt).is_err());
        let mut choices = vec![OrganizationChoice {
            id: id.clone(),
            name: "One".into(),
        }];
        assert_eq!(organization(&choices, &mut prompt).unwrap(), id);
        assert_eq!(prompt.calls, 0);
        let second = OrgId::new();
        choices.push(OrganizationChoice {
            id: second.clone(),
            name: "Two".into(),
        });
        assert_eq!(organization(&choices, &mut prompt).unwrap(), second);
        assert_eq!(prompt.calls, 1);
        assert!(prompt.labels[1].contains("Two"));
        prompt.selected = Err("selection cancelled");
        assert!(organization(&choices, &mut prompt).is_err());
        prompt.selected = Ok(100);
        assert!(organization(&choices, &mut prompt).is_err());
    }
    #[test]
    fn project_always_prompts_filters_inactive_and_keeps_duplicate_names_distinct() {
        let id = org();
        let mut prompt = Prompt {
            calls: 0,
            selected: Ok(0),
            labels: vec![],
        };
        let mut projects = vec![ProjectChoice {
            id: ProjectId::new(),
            org_id: id.clone(),
            name: "Same name".into(),
            active: true,
        }];
        assert_eq!(
            project(&id, &projects, &mut prompt).unwrap(),
            projects[0].id
        );
        assert_eq!(prompt.calls, 1);
        projects.push(ProjectChoice {
            id: ProjectId::new(),
            org_id: id.clone(),
            name: "Same name".into(),
            active: false,
        });
        project(&id, &projects, &mut prompt).unwrap();
        assert_eq!(prompt.labels.len(), 1);
        projects[1].active = true;
        prompt.selected = Ok(1);
        assert_eq!(
            project(&id, &projects, &mut prompt).unwrap(),
            projects[1].id
        );
        assert_ne!(prompt.labels[0], prompt.labels[1]);
        projects[1].org_id = OrgId::new();
        prompt.selected = Ok(0);
        project(&id, &projects, &mut prompt).unwrap();
        assert_eq!(prompt.labels.len(), 1);
        prompt.selected = Err("selection cancelled");
        assert!(project(&id, &projects, &mut prompt).is_err());
        assert!(project(&id, &[], &mut prompt).is_err());
    }
}
