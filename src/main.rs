use bootleg_cli::{
    application::{
        ProjectCredentialDelivery, complete_project_issuance, require_rotation_confirmation,
    },
    config::{RepositoryConfig, resolve_origin},
    display,
    domain::{ApiOrigin, DeviceLogin, NoticeMode, PolicyError, terminal_text},
    git,
    http::{ApiClient, Failure},
    storage::{CredentialStore, create_secret_file},
};
use bootleg_cli::{
    fuzzy::Fzf,
    selection::{self, OrganizationChoice, ProjectChoice},
};
use bootleg_core::{DisplayName, OrgId, ProjectId, SecretString, api::*};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::{
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use display::style::{self, Renderer, Tone};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Parser)]
#[command(
    name = "bootleg",
    version,
    about = "Discover and manage your Bootleg containers."
)]
struct Cli {
    #[arg(long, global = true, env = "BOOTLEG_API_URL")]
    api_url: Option<String>,
    #[arg(long, global = true, env = "BOOTLEG_ALLOW_HTTP")]
    allow_http: bool,
    #[arg(long, global = true)]
    json: bool,
    /// Disable human-output colors (JSON is never styled).
    #[arg(long, global = true)]
    no_color: bool,
    #[arg(long, global = true, env = "BOOTLEG_NOTICES", default_value = "auto")]
    notices: NoticeMode,
    #[arg(long, global = true, default_value = ".bootleg.json")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Authorize this CLI in your browser (credentials last 30 days).
    Login {
        /// Print the approval URL without launching a browser (for remote terminals).
        #[arg(long)]
        no_browser: bool,
        /// Label shown in the website's authorized-device list.
        #[arg(long)]
        device_name: Option<DisplayName>,
    },
    Logout,
    /// Show the authenticated user and credential expiry.
    Whoami,
    /// Select an organization and project interactively using fzf.
    Config,
    Orgs {
        #[command(subcommand)]
        command: OrgsCommand,
    },
    Projects {
        #[command(subcommand)]
        command: ProjectsCommand,
    },
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    Init {
        #[arg(long)]
        project_id: ProjectId,
        #[arg(long)]
        org_id: Option<OrgId>,
    },
    Instances {
        #[command(subcommand)]
        command: InstancesCommand,
    },
}
#[derive(Subcommand)]
enum OrgsCommand {
    /// List organizations accessible to your user.
    List,
    /// Save an organization selection and clear the selected project.
    Use { org_id: OrgId },
}
#[derive(Subcommand)]
enum ProjectsCommand {
    /// List projects in the explicit or saved organization.
    List {
        #[arg(long)]
        org_id: Option<OrgId>,
    },
    Create {
        name: DisplayName,
        #[arg(long)]
        org_id: Option<OrgId>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Save an active project selection in the explicit or saved organization.
    Use {
        project_id: ProjectId,
        #[arg(long)]
        org_id: Option<OrgId>,
    },
}
#[derive(Subcommand)]
enum ProjectCommand {
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
}
#[derive(Subcommand)]
enum TokenCommand {
    Rotate {
        #[arg(long)]
        project_id: Option<ProjectId>,
        #[arg(long)]
        confirm: bool,
        #[arg(long)]
        token_file: PathBuf,
    },
}
#[derive(Subcommand)]
enum InstancesCommand {
    List {
        #[arg(long)]
        project_id: Option<ProjectId>,
        #[arg(long)]
        watch: bool,
    },
}
fn output(value: &impl Serialize, json: bool) -> Result<()> {
    let text = if json {
        serde_json::to_string(value)?
    } else {
        style::result(&serde_json::to_value(value)?)
    };
    // serde JSON already escapes control characters. Human output remains escaped as well.
    println!("{text}");
    Ok(())
}
fn discovery_output(value: &impl Serialize, human: String, json: bool) -> Result<()> {
    if json {
        output(value, true)
    } else {
        print!("{human}");
        Ok(())
    }
}
fn config_output(config: &RepositoryConfig, cli: &Cli) -> Result<()> {
    discovery_output(
        config,
        Renderer::stdout().panel(
            "Success · Configuration saved",
            &format!(
                "API: {}\nOrganization: {}\nProject: {}\nSaved to: {}",
                config.api_url.as_deref().unwrap_or("not selected"),
                config
                    .org_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "not selected".into()),
                config
                    .project_id
                    .as_ref()
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "not selected".into()),
                cli.config.display()
            ),
            Tone::Success,
        ),
        cli.json,
    )
}

fn selected_org(value: Option<OrgId>, config: &RepositoryConfig) -> Result<OrgId> {
    value
        .or(config.org_id.clone())
        .ok_or_else(|| "select an organization with bootleg orgs use or bootleg config".into())
}
fn selected_project(value: Option<ProjectId>, config: &RepositoryConfig) -> Result<ProjectId> {
    value
        .or_else(|| config.project_id.clone())
        .ok_or_else(|| "select a project with bootleg projects use or init".into())
}
fn store() -> Result<CredentialStore> {
    let home = std::env::var_os("HOME")
        .ok_or("HOME is required for the owner-only credential file fallback")?;
    Ok(CredentialStore::new(
        PathBuf::from(home).join(".config/bootleg/credentials"),
    ))
}
fn credential(origin: &ApiOrigin) -> Result<Option<SecretString>> {
    if let Ok(token) = std::env::var("BOOTLEG_USER_TOKEN") {
        let scope = std::env::var("BOOTLEG_USER_TOKEN_ORIGIN")
            .map_err(|_| "CI token requires BOOTLEG_USER_TOKEN_ORIGIN")?;
        let scoped = ApiOrigin::parse(&scope, true)?;
        if scoped.as_str() != origin.as_str() {
            return Err("CI credential is scoped to a different API origin".into());
        }
        if token.is_empty() || token.len() > 512 || token.chars().any(char::is_control) {
            return Err("invalid CI credential".into());
        }
        return Ok(Some(SecretString::new(token)));
    }
    store()?.load(origin)
}
fn pages<T: serde::de::DeserializeOwned>(api: &ApiClient, path: &str) -> Result<Vec<T>> {
    let mut all = Vec::new();
    let mut query = vec![("limit".into(), "200".into())];
    let mut seen = std::collections::HashSet::new();
    loop {
        let page: Page<T> = api.get(path, &query)?;
        all.extend(page.items);
        match page.next_cursor {
            None => return Ok(all),
            Some(c) => {
                if !seen.insert(c.as_str().to_owned()) || seen.len() > 1000 {
                    return Err("invalid or excessive pagination".into());
                }
                query = vec![
                    ("limit".into(), "200".into()),
                    ("cursor".into(), c.as_str().into()),
                ];
            }
        }
    }
}
struct FileDelivery {
    path: PathBuf,
    file: std::fs::File,
}
impl FileDelivery {
    fn reserve(path: &Path) -> Result<Self> {
        // Reserve before requesting a one-time secret. Refuse overwrite or symlink.
        Ok(Self {
            path: path.into(),
            file: create_secret_file(path)?,
        })
    }
}
impl ProjectCredentialDelivery for FileDelivery {
    fn deliver(&mut self, key: &SecretString) -> std::result::Result<(), PolicyError> {
        self.file.write_all(key.expose_secret().as_bytes()).and_then(|_| self.file.sync_all()).map_err(|_| PolicyError("project key issued but secure delivery failed; reconcile and explicitly rotate if necessary"))
    }
}
impl Drop for FileDelivery {
    fn drop(&mut self) {
        if self.file.metadata().is_ok_and(|m| m.len() == 0) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
fn login(
    api: &ApiClient,
    origin: &ApiOrigin,
    allow_http: bool,
    no_browser: bool,
    device_name: Option<&DisplayName>,
) -> Result<DeviceTokenResponse> {
    // Fail local storage setup before asking the user to approve a single-use grant.
    let credentials = store()?;
    credentials.prepare()?;
    let start = Instant::now();
    let unique = bootleg_core::DeviceGrantId::new().to_string();
    let label = format!(
        "{} CLI {}",
        std::env::consts::OS,
        &unique[unique.len() - 8..]
    );
    let grant: DeviceCodeResponse = api.post(
        "auth/device/code",
        &DeviceCodeRequest {
            device_name: Some(match device_name {
                Some(name) => name.clone(),
                None => label.parse().map_err(|_| "invalid device name")?,
            }),
        },
        false,
    )?;
    let mut policy = DeviceLogin::new(grant.expires_in, grant.interval)?;
    // Browser URL may have a path but never credentials, query, fragment, or plaintext without opt-in.
    let uri =
        url::Url::parse(&grant.verification_uri).map_err(|_| "invalid device verification URL")?;
    if !(uri.scheme() == "https" || allow_http && uri.scheme() == "http")
        || uri.host_str().is_none()
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return Err("unsafe device verification URL: expected HTTPS (or explicitly allowed HTTP), a host, and no credentials, query, or fragment".into());
    }
    if uri.origin()
        != url::Url::parse(origin.as_str())
            .map_err(|_| "invalid API origin")?
            .origin()
    {
        return Err(format!(
            "device verification origin {} does not match API origin {}; configure the web server's BOOTLEG_PUBLIC_BASE_URL to match the CLI API origin and restart the server",
            terminal_text(&uri.origin().ascii_serialization()),
            terminal_text(origin.as_str()),
        ).into());
    }
    style::message(
        "Login · Approval required",
        &format!(
            "Approve Bootleg CLI at {} using code {}",
            terminal_text(&grant.verification_uri),
            terminal_text(grant.user_code.expose_secret())
        ),
        Tone::Info,
    );
    // Browser launch is best effort; the printable URL always remains available.
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer.exe"
    } else {
        "xdg-open"
    };
    if !no_browser {
        match std::process::Command::new(opener)
            .arg(uri.as_str())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    if !child.wait().is_ok_and(|status| status.success()) {
                        style::message(
                            "Warning",
                            "Could not open a browser automatically. Open the URL above to continue.",
                            Tone::Warning,
                        );
                    }
                });
            }
            Err(_) => style::message(
                "Warning",
                "Could not open a browser automatically. Open the URL above to continue.",
                Tone::Warning,
            ),
        }
    }
    loop {
        std::thread::sleep(policy.wait(start.elapsed())?);
        if start.elapsed() >= Duration::from_secs(grant.expires_in) {
            return Err("device login expired".into());
        }
        let result: std::result::Result<DeviceTokenResponse, Failure> = api.post(
            "auth/device/token",
            &DeviceTokenRequest {
                device_code: grant.device_code.clone(),
            },
            false,
        );
        match result {
            Ok(token) => {
                credentials.save(origin, token.access_token.clone()).map_err(|_| {
                    "login approved but credential storage failed; revoke this device on the website and start a fresh login"
                })?;
                return Ok(token);
            }
            Err(Failure::Server(e)) => {
                if policy
                    .rejected(start.elapsed(), e.code, e.retry_after_seconds)
                    .is_err()
                {
                    return Err(Failure::Server(e).into());
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}
fn run(cli: &Cli) -> Result<()> {
    let mut config = RepositoryConfig::load(&cli.config)?;
    let mut picker = if matches!(cli.command, Command::Config) {
        Some(Fzf::preflight()?)
    } else {
        None
    };
    let origin = resolve_origin(cli.api_url.as_deref(), &config, cli.allow_http)?;
    if let Some(saved) = &config.api_url
        && ApiOrigin::parse(saved, true)?.as_str() != origin.as_str()
    {
        config.org_id = None;
        config.project_id = None;
    }
    if let Command::Init { project_id, org_id } = &cli.command {
        config = RepositoryConfig::initialize(&origin, org_id.clone(), project_id.clone());
        config.save(&cli.config)?;
        return config_output(&config, cli);
    }
    // Fail confirmation locally, before even bootstrap or credential loading.
    if let Command::Project {
        command:
            ProjectCommand::Token {
                command: TokenCommand::Rotate { confirm, .. },
            },
    } = &cli.command
    {
        require_rotation_confirmation(*confirm)?;
    }
    // Resolve required local selection before credentials or network activity.
    if let Command::Projects {
        command: ProjectsCommand::List { org_id } | ProjectsCommand::Use { org_id, .. },
    } = &cli.command
    {
        selected_org(org_id.clone(), &config)?;
    }
    let token = if matches!(cli.command, Command::Login { .. }) {
        None
    } else {
        credential(&origin)?
    };
    let has_credential = token.is_some();
    if !has_credential
        && matches!(
            cli.command,
            Command::Whoami
                | Command::Orgs { .. }
                | Command::Projects {
                    command: ProjectsCommand::List { .. } | ProjectsCommand::Use { .. }
                }
        )
    {
        return Err("not logged in; run bootleg login".into());
    }
    let mut api = ApiClient::new(origin.clone(), token)?;
    if cli.notices != NoticeMode::Never {
        let bootstrap = api.bootstrap();
        for message in bootleg_cli::application::notice_diagnostics(
            bootstrap.as_ref(),
            cli.notices,
            cli.json,
            std::io::stderr().is_terminal(),
        ) {
            style::message("Notice", &message, Tone::Info);
        }
    }
    match &cli.command {
        Command::Login {
            no_browser,
            device_name,
        } => {
            let token = login(
                &api,
                &origin,
                cli.allow_http,
                *no_browser,
                device_name.as_ref(),
            )?;
            output(
                &serde_json::json!({"logged_in":true,"expires_in":token.expires_in}),
                cli.json,
            )
        }
        Command::Config => {
            let needs_login = if has_credential {
                match api.get::<IdentityResponse>("auth/whoami", &[]) {
                    Ok(_) => false,
                    Err(Failure::Server(e)) if e.code == ErrorCode::Unauthorized => true,
                    Err(e) => return Err(e.into()),
                }
            } else {
                true
            };
            if needs_login {
                style::message(
                    "Login",
                    "You are not logged in, or your login has expired. Starting browser login before configuring this project.",
                    Tone::Info,
                );
                let token = login(&api, &origin, cli.allow_http, false, None)?;
                api = ApiClient::new(origin.clone(), Some(token.access_token))?;
                style::message(
                    "Success",
                    "Logged in. Loading your organizations and projects.",
                    Tone::Success,
                );
                if std::env::var_os("BOOTLEG_USER_TOKEN").is_some() {
                    style::message(
                        "Warning",
                        "Replace or unset the expired BOOTLEG_USER_TOKEN for subsequent commands.",
                        Tone::Warning,
                    );
                }
            }
            let organizations: Vec<Organization> = pages(&api, "orgs")?;
            let choices: Vec<_> = organizations
                .into_iter()
                .map(|o| OrganizationChoice {
                    id: o.org_id,
                    name: o.name.as_str().into(),
                })
                .collect();
            let picker = picker.as_mut().expect("interactive preflight completed");
            let org_id = selection::organization(&choices, picker)?;
            if choices.len() == 1 {
                style::message(
                    "Organization selected",
                    &format!(
                        "Using organization {} ({org_id}).",
                        terminal_text(&choices[0].name)
                    ),
                    Tone::Info,
                );
            }
            let projects: Vec<Project> = pages(&api, &format!("orgs/{org_id}/projects"))?;
            let choices: Vec<_> = projects
                .into_iter()
                .map(|p| ProjectChoice {
                    id: p.project_id,
                    org_id: p.org_id,
                    name: p.name.as_str().into(),
                    active: p.active,
                })
                .collect();
            let project_id = selection::project(&org_id, &choices, picker)?;
            config = RepositoryConfig::initialize(&origin, Some(org_id), project_id);
            config.save(&cli.config)?;
            config_output(&config, cli)
        }
        Command::Logout => {
            match api.logout() {
                Ok(()) => {}
                Err(Failure::Server(e)) if e.code == ErrorCode::Unauthorized => {}
                Err(e) => return Err(e.into()),
            }
            store()?.remove(&origin)?;
            output(
                &serde_json::json!({"logged_out": true, "note": "CI environment credentials must also be removed from your environment"}),
                cli.json,
            )
        }
        Command::Whoami => {
            let identity = api.get::<IdentityResponse>("auth/whoami", &[])?;
            let color = Renderer::stdout().color;
            discovery_output(
                &identity,
                display::identity_with_color(&identity, color),
                cli.json,
            )
        }
        Command::Orgs { command } => {
            let orgs: Vec<Organization> = pages(&api, "orgs")?;
            match command {
                OrgsCommand::List => discovery_output(
                    &orgs,
                    display::organizations(&orgs, config.org_id.as_ref()),
                    cli.json,
                ),
                OrgsCommand::Use { org_id } => {
                    let selected = orgs
                        .iter()
                        .find(|o| o.org_id == *org_id)
                        .ok_or("organization not eligible")?;
                    config.org_id = Some(org_id.clone());
                    config.project_id = None;
                    config.api_url = Some(origin.as_str().into());
                    config.save(&cli.config)?;
                    discovery_output(
                        selected,
                        Renderer::stdout().panel("Success · Organization selected", &format!("Using organization {} ({org_id}). Project selection cleared.\nSaved to: {}", terminal_text(selected.name.as_str()), cli.config.display()), Tone::Success),
                        cli.json,
                    )
                }
            }
        }
        Command::Projects { command } => match command {
            ProjectsCommand::List { org_id } => {
                let org = selected_org(org_id.clone(), &config)?;
                let projects = pages::<Project>(&api, &format!("orgs/{org}/projects"))?;
                let current = if config.org_id.as_ref() == Some(&org) {
                    config.project_id
                } else {
                    None
                };
                discovery_output(&projects, display::projects(&projects, current), cli.json)
            }
            ProjectsCommand::Use { project_id, org_id } => {
                let org = selected_org(org_id.clone(), &config)?;
                let projects: Vec<Project> = pages(&api, &format!("orgs/{org}/projects"))?;
                let selected =
                    bootleg_cli::application::select_project(&projects, &org, project_id.clone())?;
                config.org_id = Some(org);
                config.project_id = Some(project_id.clone());
                config.api_url = Some(origin.as_str().into());
                config.save(&cli.config)?;
                discovery_output(
                    selected,
                    Renderer::stdout().panel(
                        "Success · Project selected",
                        &format!(
                            "Using project {} ({project_id}) in organization {}.\nSaved to: {}",
                            terminal_text(selected.name.as_str()),
                            selected.org_id,
                            cli.config.display()
                        ),
                        Tone::Success,
                    ),
                    cli.json,
                )
            }
            ProjectsCommand::Create {
                name,
                org_id,
                url,
                token_file,
            } => {
                if let Some(value) = url {
                    let u = url::Url::parse(value).map_err(|_| "invalid project URL")?;
                    if value.len() > 2048
                        || !matches!(u.scheme(), "http" | "https")
                        || !u.username().is_empty()
                        || u.password().is_some()
                    {
                        return Err("project URL must be credential-free HTTP(S)".into());
                    }
                }
                let mut delivery = FileDelivery::reserve(token_file)?;
                let response: ProjectTokenResponse = api.post(
                    &format!("orgs/{}/projects", selected_org(org_id.clone(), &config)?),
                    &CreateProjectRequest {
                        name: name.clone(),
                        url: url.clone(),
                    },
                    true,
                )?;
                let project = complete_project_issuance(response, &mut delivery)?;
                discovery_output(&project, Renderer::stdout().panel("Success · Project created", &format!("Project: {} ({})\nToken file: {}\nKeep this file outside Git and deliver it using your secret manager.", project.name.as_str(), project.project_id, token_file.display()), Tone::Success), cli.json)
            }
        },
        Command::Project {
            command:
                ProjectCommand::Token {
                    command:
                        TokenCommand::Rotate {
                            project_id,
                            token_file,
                            ..
                        },
                },
        } => {
            let mut delivery = FileDelivery::reserve(token_file)?;
            let response: ProjectTokenResponse = api.post(
                &format!(
                    "projects/{}/token/rotate",
                    selected_project(project_id.clone(), &config)?
                ),
                &RotateProjectTokenRequest::default(),
                true,
            )?;
            let project = complete_project_issuance(response, &mut delivery)?;
            discovery_output(&project, Renderer::stdout().panel("Success · Project token rotated", &format!("Project: {} ({})\nToken file: {}\nPrevious agent keys are invalid. Deliver the new key using your secret manager.", project.name.as_str(), project.project_id, token_file.display()), Tone::Success), cli.json)
        }
        Command::Instances {
            command: InstancesCommand::List { project_id, watch },
        } => {
            let id = selected_project(project_id.clone(), &config)?;
            loop {
                let mut query = vec![("limit".into(), "200".into())];
                let mut items = Vec::new();
                let mut seen = std::collections::HashSet::new();
                let server_time;
                loop {
                    let page: InstancesResponse = match api
                        .get(&format!("projects/{id}/instances"), &query)
                    {
                        Ok(page) => page,
                        Err(e) => {
                            if *watch {
                                // Never leave a failed refresh looking like current Presence.
                                output(
                                    &serde_json::json!({"stale":true,"items":null,"error":e.json()["error"]}),
                                    cli.json,
                                )?;
                            }
                            return Err(e.into());
                        }
                    };
                    items.extend(page.items.into_iter().map(|c| {
                        let baseline =
                            git::inspect(Path::new("."), c.build_commit.as_ref(), c.build_dirty);
                        serde_json::json!({"container": c, "baseline": baseline})
                    }));
                    match page.next_cursor {
                        None => {
                            server_time = page.server_time;
                            break;
                        }
                        Some(c) => {
                            if !seen.insert(c.as_str().to_string()) || seen.len() > 1000 {
                                return Err("invalid snapshot pagination".into());
                            }
                            query = vec![
                                ("limit".into(), "200".into()),
                                ("cursor".into(), c.as_str().into()),
                            ];
                        }
                    }
                }
                output(
                    &serde_json::json!({"items":items,"server_time":server_time,"stale":false,"presence_meaning":"control-plane connectivity, not readiness"}),
                    cli.json,
                )?;
                if !watch {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        }
        _ => unreachable!("offline commands handled above"),
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let cli = if args.len() == 1 {
        Cli::parse_from(args.into_iter().chain([std::ffi::OsString::from("--help")]))
    } else {
        Cli::parse_from(args)
    };
    style::configure(cli.no_color);
    if let Err(e) = run(&cli) {
        if cli.json {
            let value = e.downcast_ref::<Failure>().map(Failure::json).unwrap_or_else(|| serde_json::json!({"error":{"code":"cli_error","message":terminal_text(&e.to_string()),"level":"error"}}));
            println!("{}", value);
        } else {
            style::message("Error", &e.to_string(), Tone::Error);
        }
        std::process::exit(1);
    }
}
