//! Real executable tests: no API server, no network mocks.
use std::process::Command;
fn cli(root: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_bootleg"));
    c.current_dir(root)
        .env_remove("BOOTLEG_API_URL")
        .env_remove("BOOTLEG_USER_TOKEN")
        .env_remove("BOOTLEG_NOTICES")
        .env_remove("BOOTLEG_ALLOW_HTTP");
    c
}
#[test]
fn offline_init_and_optional_agent_snippets() {
    let t = tempfile::tempdir().unwrap();
    let init = cli(t.path())
        .args([
            "--api-url",
            "https://unreachable.invalid",
            "--json",
            "init",
            "--project-id",
            "42",
        ])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config: serde_json::Value = serde_json::from_slice(&init.stdout).unwrap();
    assert_eq!(config["project_id"], "42");
    let generated = cli(t.path())
        .env("MY_POD", "laptop-secret-not-a-label")
        .args(["--json", "generate", "kubernetes", "--name-env", "MY_POD"])
        .output()
        .unwrap();
    assert!(generated.status.success());
    let output: serde_json::Value = serde_json::from_slice(&generated.stdout).unwrap();
    let snippet = output["snippet"].as_str().unwrap();
    assert!(snippet.contains("MY_POD"));
    assert!(!snippet.contains("laptop-secret"));
    assert!(generated.stderr.is_empty());
    assert!(config.get("name_env").is_none());
    assert!(
        !cli(t.path())
            .arg("baseline")
            .output()
            .unwrap()
            .status
            .success()
    );
}
#[test]
fn rotation_requires_confirmation_before_network_and_credentials() {
    let t = tempfile::tempdir().unwrap();
    let result = cli(t.path())
        .args([
            "--api-url",
            "https://unreachable.invalid",
            "--json",
            "project",
            "token",
            "rotate",
            "--project-id",
            "42",
            "--token-file",
            "secret",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("--confirm"));
    assert!(!t.path().join("secret").exists());
}
#[test]
fn explicit_flag_overrides_environment_and_invalid_notice_is_rejected() {
    let t = tempfile::tempdir().unwrap();
    let result = cli(t.path())
        .env("BOOTLEG_API_URL", "https://env.invalid")
        .args([
            "--api-url",
            "https://flag.invalid",
            "--json",
            "init",
            "--project-id",
            "1",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()["api_url"],
        "https://flag.invalid/"
    );
    let invalid = cli(t.path())
        .args(["--notices", "invalid", "orgs", "list"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

#[test]
fn orgs_command_and_config_preflight() {
    let t = tempfile::tempdir().unwrap();
    for args in [["orgs", "--help"], ["config", "--help"]] {
        let output = cli(t.path()).args(args).output().unwrap();
        assert!(output.status.success());
    }
    assert!(
        !cli(t.path())
            .args(["workspaces", "list"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let original = b"{\"api_url\":\"https://unreachable.invalid\",\"project_id\":\"42\"}";
    std::fs::write(t.path().join(".bootleg.json"), original).unwrap();
    let output = cli(t.path()).args(["--json", "config"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("interactive terminal"));
    assert_eq!(
        std::fs::read(t.path().join(".bootleg.json")).unwrap(),
        original
    );
}
