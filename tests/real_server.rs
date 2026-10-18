//! Opt-in tests against the actual web service and its real backing stores.
//! Fixture credentials/mock servers are expressly not used.
use bootleg_cli::{
    domain::ApiOrigin,
    http::{ApiClient, Failure},
};
use bootleg_core::{
    SecretString,
    api::{ErrorCode, IdentityResponse, InstancesResponse},
};
fn client(variable: &str) -> ApiClient {
    let origin = ApiOrigin::parse(
        &std::env::var("BOOTLEG_TEST_API_URL").expect("real web API URL"),
        std::env::var("BOOTLEG_TEST_ALLOW_HTTP").as_deref() == Ok("true"),
    )
    .unwrap();
    ApiClient::new(
        origin,
        Some(SecretString::new(
            std::env::var(variable).expect("real provisioned user credential"),
        )),
    )
    .unwrap()
}
#[test]
#[ignore = "requires actual web server and an expired/revoked user credential"]
fn expired_credential_is_unauthorized() {
    let result =
        client("BOOTLEG_TEST_EXPIRED_USER_TOKEN").get::<IdentityResponse>("auth/whoami", &[]);
    assert!(matches!(result, Err(Failure::Server(e)) if e.code == ErrorCode::Unauthorized));
}
#[test]
#[ignore = "requires actual web server, user credential, and another organization's project"]
fn cross_org_project_is_not_found() {
    let project: bootleg_core::ProjectId = std::env::var("BOOTLEG_TEST_HIDDEN_PROJECT_ID")
        .unwrap()
        .parse()
        .unwrap();
    let result = client("BOOTLEG_TEST_USER_TOKEN")
        .get::<InstancesResponse>(&format!("projects/{project}/instances"), &[]);
    assert!(matches!(result, Err(Failure::Server(e)) if e.code == ErrorCode::NotFound));
}
#[test]
#[ignore = "requires actual web server and eligible user credential"]
fn bootstrap_and_identity_use_real_contract() {
    let api = client("BOOTLEG_TEST_USER_TOKEN");
    assert_eq!(
        api.bootstrap()
            .expect("bootstrap from real web")
            .protocol_version,
        1
    );
    api.get::<IdentityResponse>("auth/whoami", &[]).unwrap();
}
