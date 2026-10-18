use crate::domain::{ApiOrigin, PolicyError};
use bootleg_core::ProjectId;

pub fn validate_name_env(name: &str) -> Result<(), PolicyError> {
    let mut b = name.bytes();
    if !b
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        || !b.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        || name.len() > 256
    {
        return Err(PolicyError("name-env must be an environment variable name"));
    }
    Ok(())
}
/// Generates references only. Never reads the developer's selected variable or a project token.
pub fn generate(
    origin: &ApiOrigin,
    project: ProjectId,
    name_env: Option<&str>,
    kubernetes: bool,
) -> Result<String, PolicyError> {
    if let Some(name) = name_env {
        validate_name_env(name)?;
    }
    let quote = |s: &str| serde_json::to_string(s).expect("string serialization");
    let mut out = String::from(
        "# Optional smuggler deployment configuration, not a CLI setup or build step.\n# Assumes your container already runs smuggler as its entrypoint.\n# Discovery only: Presence is not readiness or file synchronization.\n# Smuggler resolves pod names inside the container; the CLI does not read them.\n# Inject project credentials at runtime only, never into an image.\n",
    );
    if kubernetes {
        out.push_str("# Merge into your existing container; preserve application probes. Do not auto-apply.\nenv:\n");
        for (key, value) in [
            ("BOOTLEG_API_URL", origin.as_str().to_string()),
            ("BOOTLEG_PROJECT_ID", project.to_string()),
            ("BOOTLEG_CONTAINER_NAME", "app".into()),
        ] {
            out.push_str(&format!("  - name: {key}\n    value: {}\n", quote(&value)));
        }
        if let Some(name) = name_env {
            out.push_str(&format!(
                "  - name: BOOTLEG_NAME_ENV\n    value: {}\n",
                quote(name)
            ));
        }
        for (key, field) in [
            ("POD_NAME", "metadata.name"),
            ("POD_UID", "metadata.uid"),
            ("POD_NAMESPACE", "metadata.namespace"),
        ] {
            out.push_str(&format!(
                "  - name: {key}\n    valueFrom:\n      fieldRef:\n        fieldPath: {field}\n"
            ));
        }
        out.push_str("  - name: BOOTLEG_PROJECT_TOKEN\n    valueFrom:\n      secretKeyRef:\n        name: bootleg-project\n        key: token\n# Provision bootleg-project separately using your secret manager.\n");
    } else {
        out.push_str("# Docker Compose fragment; token is supplied from the runtime environment.\nservices:\n  app:\n    image: your-instrumented-app\n    environment:\n");
        out.push_str(&format!("      BOOTLEG_API_URL: {}\n      BOOTLEG_PROJECT_ID: {}\n      BOOTLEG_PROJECT_TOKEN: \"${{BOOTLEG_PROJECT_TOKEN:?inject at runtime}}\"\n", quote(origin.as_str()), quote(&project.to_string())));
        if let Some(name) = name_env {
            out.push_str(&format!("      BOOTLEG_NAME_ENV: {}\n", quote(name)));
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn references_not_laptop_values() {
        let api = ApiOrigin::parse("https://example.com", false).unwrap();
        for kube in [true, false] {
            let out = generate(&api, "42".parse().unwrap(), Some("POD_NAME"), kube).unwrap();
            assert!(out.contains("BOOTLEG_NAME_ENV"));
            assert!(out.contains("\"POD_NAME\""));
            assert!(!out.contains("secret_hash"));
            if kube {
                for field in [
                    "metadata.name",
                    "metadata.uid",
                    "metadata.namespace",
                    "secretKeyRef",
                ] {
                    assert!(out.contains(field));
                }
            }
        }
        for bad in ["", "1NAME", "A\nB", "A;evil", "${TOKEN}"] {
            assert!(validate_name_env(bad).is_err());
        }
    }
}
