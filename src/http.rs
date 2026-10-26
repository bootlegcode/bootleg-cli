//! HTTPS adapter. Redirects and automatic retries are disabled, including mutations.
mod preview;
pub use preview::InstanceDiscovery;

use crate::domain::{ApiOrigin, terminal_text};
use bootleg_core::{
    SecretString,
    api::{ApiError, ErrorResponse},
    communication::BootstrapResponse,
};
use reqwest::{Method, blocking::Client};
use serde::{Serialize, de::DeserializeOwned};
use std::{io::Read, time::Duration};

#[derive(Debug)]
pub enum Failure {
    Server(ApiError),
    Local(&'static str),
    Transport(String),
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Server(e) => write!(
                f,
                "{:?}: {} (request {})",
                e.level,
                terminal_text(e.message.as_str()),
                terminal_text(e.request_id.as_str())
            ),
            Self::Local(s) => f.write_str(s),
            Self::Transport(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for Failure {}
impl Failure {
    pub fn json(&self) -> serde_json::Value {
        match self {
            Self::Server(e) => serde_json::json!({"error": e}),
            Self::Local(s) => {
                serde_json::json!({"error": {"code": "cli_error", "level": "error", "message": s}})
            }
            Self::Transport(s) => {
                serde_json::json!({"error": {"code": "cli_error", "level": "error", "message": s}})
            }
        }
    }
}
pub struct ApiClient {
    client: Client,
    origin: ApiOrigin,
    token: Option<SecretString>,
}
impl ApiClient {
    pub fn new(origin: ApiOrigin, token: Option<SecretString>) -> Result<Self, Failure> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| Failure::Local("cannot initialize HTTPS client"))?;
        Ok(Self {
            client,
            origin,
            token,
        })
    }
    pub fn bootstrap(&self) -> Option<BootstrapResponse> {
        self.request::<BootstrapResponse, ()>(
            Method::GET,
            "client/bootstrap",
            None,
            &[],
            false,
            Duration::from_secs(2),
        )
        .ok()
        .filter(|b| b.protocol_version == 1)
    }
    pub fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<T, Failure> {
        self.request::<T, ()>(
            Method::GET,
            path,
            None,
            query,
            true,
            Duration::from_secs(30),
        )
    }
    pub fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
        authenticated: bool,
    ) -> Result<T, Failure> {
        self.request(
            Method::POST,
            path,
            Some(body),
            &[],
            authenticated,
            Duration::from_secs(30),
        )
    }
    pub fn logout(&self) -> Result<(), Failure> {
        self.request::<serde_json::Value, ()>(
            Method::POST,
            "auth/logout",
            None,
            &[],
            true,
            Duration::from_secs(30),
        )
        .map(|_| ())
    }
    fn request<T: DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        query: &[(String, String)],
        authenticated: bool,
        timeout: Duration,
    ) -> Result<T, Failure> {
        let mutation = method != Method::GET;
        let request_label = format!("{method} {}api/v1/{path}", self.origin.as_str());
        let mut req = self
            .client
            .request(method, format!("{}api/v1/{path}", self.origin.as_str()))
            .header("Bootleg-Client", "cli")
            .header("Bootleg-Client-Version", env!("CARGO_PKG_VERSION"))
            .header("Bootleg-Protocol-Version", "1")
            .header("Accept", "application/json")
            .timeout(timeout)
            .query(query);
        if authenticated {
            req = req.bearer_auth(
                self.token
                    .as_ref()
                    .ok_or(Failure::Local("not logged in for this API origin"))?
                    .expose_secret(),
            );
        }
        if let Some(body) = body {
            let bytes =
                serde_json::to_vec(body).map_err(|_| Failure::Local("cannot encode request"))?;
            if bytes.len() > 16384 {
                return Err(Failure::Local("request exceeds protocol limit"));
            }
            req = req.header("Content-Type", "application/json").body(bytes);
        }
        let recovery = transport_recovery(path, mutation);
        let response = req.send().map_err(|e| {
            let reason = if e.is_connect() {
                "could not connect (check the API URL, server availability, DNS, and TLS)"
            } else if e.is_timeout() {
                "request timed out"
            } else {
                "request transport failed"
            };
            Failure::Transport(format!(
                "{request_label}: {reason}: {}. {recovery}",
                terminal_text(&e.without_url().to_string())
            ))
        })?;
        let status = response.status();
        if response
            .headers()
            .get("Bootleg-Protocol-Version")
            .and_then(|v| v.to_str().ok())
            != Some("1")
        {
            return Err(Failure::Local(
                "missing or incompatible API protocol header; mutation may have completed",
            ));
        }
        let mut bytes = Vec::new();
        response
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| {
                Failure::Transport(format!(
                    "{request_label}: failed to read the API response (HTTP {status}). {recovery}"
                ))
            })?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(Failure::Local(
                "API response exceeds client limit; mutation may have completed",
            ));
        }
        if !status.is_success() {
            return Err(serde_json::from_slice::<ErrorResponse>(&bytes)
                .map(|r| Failure::Server(r.error))
                .unwrap_or(Failure::Local(
                    "invalid API error response; mutation may have completed",
                )));
        }
        if status.as_u16() == 204 {
            bytes = b"null".to_vec();
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| Failure::Local("invalid API response; mutation may have completed"))
    }
}
fn transport_recovery(path: &str, mutation: bool) -> &'static str {
    match path {
        "auth/device/code" => {
            "Could not start device login. Check connectivity and the API URL, then run `bootleg login` again."
        }
        "auth/device/token" => {
            "Could not confirm login; approval may have completed. Revoke this device on the website if it was approved, then start a fresh `bootleg login`."
        }
        _ if mutation => {
            "The server may have completed this operation. Check server state before retrying; this request was not automatically retried."
        }
        _ => "Check connectivity and the API URL, then try again.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_errors_preserve_context_in_both_outputs() {
        let message = format!(
            "POST https://example.com/api/v1/auth/device/code: could not connect. {}",
            transport_recovery("auth/device/code", true)
        );
        let failure = Failure::Transport(message.clone());
        assert_eq!(failure.to_string(), message);
        assert_eq!(failure.json()["error"]["message"], message);
        assert!(!message.contains("mutation"));
    }

    #[test]
    fn recovery_distinguishes_login_exchange_and_mutations() {
        assert!(transport_recovery("auth/device/token", true).contains("Revoke this device"));
        assert!(transport_recovery("projects", true).contains("Check server state"));
        assert!(!transport_recovery("projects", false).contains("may have completed"));
    }

    #[test]
    fn server_copy_is_preserved_in_both_outputs() {
        let response: ErrorResponse = serde_json::from_str(r#"{"error":{"code":"service_unavailable","level":"warning","message":"New maintenance wording.","request_id":"req-1","retry_after_seconds":30}}"#).unwrap();
        let f = Failure::Server(response.error);
        assert!(f.to_string().contains("New maintenance wording."));
        assert_eq!(f.json()["error"]["message"], "New maintenance wording.");
    }
}
