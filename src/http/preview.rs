//! Bounded, GET-only authorized discovery for preview. No server text or DTO is
//! returned to the renderer. Unknown fields (including runtime data) are skipped.
use super::ApiClient;
use crate::{
    preview::{Code, Target},
    preview_application::{AuthorizedDiscovery, DiscoverySink},
};
use bootleg_core::{BuildCommit, ContainerId, Cursor, ProjectId, protocol::PresenceStatus};
use serde::{
    Deserialize, Deserializer,
    de::{SeqAccess, Visitor},
};
use std::{
    collections::BTreeSet,
    fmt,
    io::Read,
    time::{Duration, Instant},
};

pub struct InstanceDiscovery<'a> {
    api: &'a ApiClient,
    project: ProjectId,
}
impl<'a> InstanceDiscovery<'a> {
    pub fn new(api: &'a ApiClient, project: ProjectId) -> Self {
        Self { api, project }
    }
}

// Only retain preview-relevant fields, not labels, runtime or session IDs.
#[derive(Deserialize)]
struct Record {
    project_id: ProjectId,
    container_id: ContainerId,
    build_commit: Option<BuildCommit>,
    build_dirty: Option<bool>,
    presence: PresenceStatus,
}
#[derive(Deserialize)]
struct Page {
    #[serde(deserialize_with = "records")]
    items: Vec<Record>,
    // Require the pagination field even when null: a missing field must not
    // silently turn an incomplete response into a successful traversal.
    #[serde(deserialize_with = "Option::deserialize")]
    next_cursor: Option<Cursor>,
}
fn records<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Record>, D::Error> {
    struct Records;
    impl<'de> Visitor<'de> for Records {
        type Value = Vec<Record>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("at most 200 instance records")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut result = Vec::new();
            while result.len() < 200 {
                match seq.next_element()? {
                    Some(record) => result.push(record),
                    None => return Ok(result),
                }
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(
                    "instance page exceeds requested limit",
                ));
            }
            Ok(result)
        }
    }
    d.deserialize_seq(Records)
}
fn remaining(deadline: Instant) -> Result<Duration, Code> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .map(|d| d.min(Duration::from_secs(30)))
        .ok_or(Code::LimitExceeded)
}
fn body(
    reader: &mut impl Read,
    sink: &mut DiscoverySink,
    deadline: Instant,
) -> Result<Vec<u8>, Code> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        remaining(deadline)?;
        let n = reader.read(&mut chunk).map_err(|_| Code::DiscoveryFailed)?;
        remaining(deadline)?;
        if n == 0 {
            return Ok(bytes);
        }
        sink.response_bytes(n)?;
        bytes.try_reserve(n).map_err(|_| Code::LimitExceeded)?;
        bytes.extend_from_slice(&chunk[..n]);
    }
}
impl AuthorizedDiscovery for InstanceDiscovery<'_> {
    fn traverse(&mut self, sink: &mut DiscoverySink, deadline: Instant) -> Result<(), Code> {
        let token = self.api.token.as_ref().ok_or(Code::DiscoveryFailed)?;
        let url = format!(
            "{}api/v1/projects/{}/instances",
            self.api.origin.as_str(),
            self.project
        );
        let mut cursor: Option<Cursor> = None;
        let mut seen = BTreeSet::new();
        loop {
            sink.begin_page()?;
            let mut query = vec![("limit", "200")];
            if let Some(c) = &cursor {
                query.push(("cursor", c.as_str()));
            }
            // ApiClient disables redirects and retries. This path never calls
            // bootstrap, mutation, generic unbounded decoding or token refresh.
            let mut response = self
                .api
                .client
                .get(&url)
                .header("Bootleg-Client", "cli")
                .header("Bootleg-Client-Version", env!("CARGO_PKG_VERSION"))
                .header("Bootleg-Protocol-Version", "1")
                .header("Accept", "application/json")
                .bearer_auth(token.expose_secret())
                .query(&query)
                .timeout(remaining(deadline)?)
                .send()
                .map_err(|_| Code::DiscoveryFailed)?;
            remaining(deadline)?;
            if !response.status().is_success()
                || response
                    .headers()
                    .get("Bootleg-Protocol-Version")
                    .and_then(|v| v.to_str().ok())
                    != Some("1")
            {
                // Do not decode or expose arbitrary error bodies.
                return Err(Code::DiscoveryFailed);
            }
            let bytes = body(&mut response, sink, deadline)?;
            let page: Page = serde_json::from_slice(&bytes).map_err(|_| Code::DiscoveryFailed)?;
            remaining(deadline)?;
            for record in page.items {
                if record.project_id != self.project {
                    return Err(Code::DiscoveryFailed);
                }
                sink.record(Target {
                    container_id: record.container_id,
                    build_commit: record.build_commit,
                    build_dirty: record.build_dirty,
                    connected: record.presence == PresenceStatus::Connected,
                })?;
            }
            match page.next_cursor {
                None => return Ok(()),
                Some(next) => {
                    if !seen.insert(next.as_str().to_owned()) {
                        return Err(Code::DiscoveryFailed);
                    }
                    cursor = Some(next);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_application::{INVOCATION_TIMEOUT, MAX_DISCOVERY_BYTES};
    #[test]
    fn body_budget_is_cumulative_and_checked_before_retention() {
        let mut sink = DiscoverySink::new(&[ContainerId::new()]).unwrap();
        sink.begin_page().unwrap();
        sink.response_bytes(MAX_DISCOVERY_BYTES - 3).unwrap();
        let deadline = Instant::now() + INVOCATION_TIMEOUT;
        assert_eq!(body(&mut &b"abc"[..], &mut sink, deadline).unwrap(), b"abc");
        sink.begin_page().unwrap();
        assert_eq!(
            body(&mut &b"x"[..], &mut sink, deadline),
            Err(Code::LimitExceeded)
        );
    }
    #[test]
    fn expired_body_does_not_read() {
        struct NoRead;
        impl Read for NoRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("read after deadline")
            }
        }
        let mut sink = DiscoverySink::new(&[ContainerId::new()]).unwrap();
        sink.begin_page().unwrap();
        assert_eq!(
            body(&mut NoRead, &mut sink, Instant::now()),
            Err(Code::LimitExceeded)
        );
    }
    #[test]
    fn pagination_is_required_and_duplicate_fields_rejected() {
        assert!(serde_json::from_str::<Page>(r#"{"items":[]}"#).is_err());
        assert!(
            serde_json::from_str::<Page>(r#"{"items":[],"next_cursor":null,"next_cursor":null}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<Page>(
                r#"{"items":[],"next_cursor":null,"runtime":{"secret":"never retained"}}"#
            )
            .is_ok()
        );
    }
    #[test]
    fn record_projection_skips_secret_bearing_fields_and_bounds_page() {
        let value = serde_json::json!({
            "project_id": ProjectId::new(), "container_id": ContainerId::new(),
            "build_commit": null, "build_dirty": false, "presence": "connected",
            "runtime": {"secret": "do not retain"}, "display_name": "do not retain"
        });
        for count in [200, 201] {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "items": vec![value.clone(); count], "next_cursor": null
            }))
            .unwrap();
            assert_eq!(serde_json::from_slice::<Page>(&bytes).is_ok(), count == 200);
        }
    }
    #[test]
    fn missing_credentials_fail_before_any_request() {
        let api = ApiClient::new(
            crate::domain::ApiOrigin::parse("https://example.com", false).unwrap(),
            None,
        )
        .unwrap();
        let mut discovery = InstanceDiscovery::new(&api, ProjectId::new());
        let mut sink = DiscoverySink::new(&[ContainerId::new()]).unwrap();
        assert_eq!(
            discovery.traverse(&mut sink, Instant::now() + INVOCATION_TIMEOUT),
            Err(Code::DiscoveryFailed)
        );
    }
}
