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
fn bare_command_shows_main_help_successfully() {
    let t = tempfile::tempdir().unwrap();
    let bare = cli(t.path()).output().unwrap();
    let help = cli(t.path()).arg("--help").output().unwrap();
    assert!(bare.status.success());
    assert!(bare.stderr.is_empty());
    assert_eq!(bare.stdout, help.stdout);
    assert!(String::from_utf8_lossy(&bare.stdout).contains("Usage:"));
}

#[test]
fn offline_init_and_removed_commands() {
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
    assert!(init.stderr.is_empty());
    for target in ["docker", "kubernetes"] {
        let output = cli(t.path()).args(["generate", target]).output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand 'generate'")
        );
    }
    let help = cli(t.path()).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("generate"));
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
fn login_options_validate_before_network() {
    let t = tempfile::tempdir().unwrap();
    let output = cli(t.path()).args(["login", "--help"]).output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--no-browser") && help.contains("--device-name"));
    for name in ["", " leading", "bad\nname"] {
        let output = cli(t.path())
            .args(["login", "--device-name", name])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid value"));
    }
}

#[cfg(unix)]
#[test]
fn login_rejects_unsafe_storage_before_issuing_grant() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let credentials = t.path().join(".config/bootleg/credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = cli(t.path())
        .env("HOME", t.path())
        .args([
            "--api-url",
            "https://unreachable.invalid",
            "--notices=never",
            "login",
            "--no-browser",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("private directory"));
    assert!(output.stdout.is_empty());
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
fn discovery_commands_validate_locally_without_changing_config() {
    let t = tempfile::tempdir().unwrap();
    let original = b"{\"api_url\":\"https://unreachable.invalid\"}";
    let path = t.path().join(".bootleg.json");
    std::fs::write(&path, original).unwrap();
    for args in [
        vec!["whoami"],
        vec!["orgs", "list"],
        vec!["projects", "list"],
        vec!["projects", "use", "42"],
    ] {
        let output = cli(t.path())
            .env("HOME", t.path())
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let message = error["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("bootleg login") || message.contains("select an organization"),
            "{message}"
        );
        assert!(output.stderr.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    for args in [
        vec!["whoami", "--help"],
        vec!["orgs", "list", "--help"],
        vec!["orgs", "use", "--help"],
        vec!["projects", "list", "--help"],
        vec!["projects", "use", "--help"],
    ] {
        assert!(cli(t.path()).args(args).output().unwrap().status.success());
    }
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
