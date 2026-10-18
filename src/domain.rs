//! CLI policies, independent of HTTP, Clap and filesystem adapters.
use bootleg_core::{BuildCommit, api::ErrorCode};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PolicyError(pub &'static str);

#[derive(Clone, Debug)]
pub struct ApiOrigin(url::Url);
impl ApiOrigin {
    pub fn parse(value: &str, development: bool) -> Result<Self, PolicyError> {
        let u = url::Url::parse(value).map_err(|_| PolicyError("invalid API origin"))?;
        if !(u.scheme() == "https" || development && u.scheme() == "http")
            || u.host_str().is_none()
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || u.path() != "/"
        {
            return Err(PolicyError(
                "API URL must be an HTTPS origin (HTTP requires --allow-http)",
            ));
        }
        Ok(Self(u))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeMode {
    Auto,
    Always,
    Never,
}
impl std::str::FromStr for NoticeMode {
    type Err = PolicyError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(PolicyError("notices must be auto, always or never")),
        }
    }
}
impl NoticeMode {
    pub fn show(self, json: bool, stderr_tty: bool) -> bool {
        self == Self::Always || self == Self::Auto && !json && stderr_tty
    }
}

/// Terminal controls and bidi formatting cannot impersonate locally authored output.
pub fn terminal_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() && !matches!(*c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).collect()
}

pub struct DeviceLogin {
    expires: Duration,
    interval: Duration,
    next: Duration,
}
impl DeviceLogin {
    pub fn new(expires: u64, interval: u64) -> Result<Self, PolicyError> {
        if expires == 0 || interval == 0 {
            return Err(PolicyError("invalid device login timing"));
        }
        let interval = Duration::from_secs(interval);
        Ok(Self {
            expires: Duration::from_secs(expires),
            interval,
            next: interval,
        })
    }
    pub fn wait(&self, now: Duration) -> Result<Duration, PolicyError> {
        if now >= self.expires || self.next >= self.expires {
            return Err(PolicyError("device login expired; start a fresh login"));
        }
        Ok(self.next.saturating_sub(now))
    }
    pub fn rejected(
        &mut self,
        now: Duration,
        code: ErrorCode,
        retry: Option<u64>,
    ) -> Result<(), PolicyError> {
        match code {
            ErrorCode::AuthorizationPending => {}
            ErrorCode::SlowDown => {
                self.interval = self.interval.saturating_add(Duration::from_secs(5));
                self.interval = self.interval.max(Duration::from_secs(retry.unwrap_or(0)));
            }
            ErrorCode::RateLimited => {}
            _ => return Err(PolicyError("device login stopped; start a fresh login")),
        }
        self.next = now.saturating_add(self.interval.max(Duration::from_secs(retry.unwrap_or(0))));
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineRelationship {
    Equal,
    Ancestor,
    Descendant,
    Diverged,
    Unknown,
}
#[derive(Default)]
pub struct GitEvidence {
    pub head: Option<BuildCommit>,
    pub object_present: bool,
    pub baseline_ancestor: Option<bool>,
    pub head_ancestor: Option<bool>,
}
pub fn classify(baseline: Option<&BuildCommit>, evidence: &GitEvidence) -> BaselineRelationship {
    use BaselineRelationship::*;
    let (Some(b), Some(h)) = (baseline, &evidence.head) else {
        return Unknown;
    };
    if !evidence.object_present {
        return Unknown;
    }
    if b == h {
        Equal
    } else {
        match (evidence.baseline_ancestor, evidence.head_ancestor) {
            (Some(true), _) => Ancestor,
            (_, Some(true)) => Descendant,
            (Some(false), Some(false)) => Diverged,
            _ => Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_security() {
        for bad in [
            "http://example.com",
            "https://u:p@example.com",
            "https://example.com?token=x",
            "https://example.com/#x",
            "https://example.com/api",
        ] {
            assert!(ApiOrigin::parse(bad, false).is_err());
        }
        assert!(ApiOrigin::parse("http://localhost:3000", true).is_ok());
        assert_ne!(
            ApiOrigin::parse("https://a", false).unwrap().as_str(),
            ApiOrigin::parse("https://b", false).unwrap().as_str()
        );
    }
    #[test]
    fn notice_policy_and_terminal_safety() {
        for json in [false, true] {
            for tty in [false, true] {
                assert!(!NoticeMode::Never.show(json, tty));
                assert!(NoticeMode::Always.show(json, tty));
                assert_eq!(NoticeMode::Auto.show(json, tty), !json && tty);
            }
        }
        assert_eq!(terminal_text("hello\u{202e}\x1b world"), "hello world");
    }
    #[test]
    fn device_polling_boundaries() {
        let mut p = DeviceLogin::new(30, 5).unwrap();
        assert_eq!(p.wait(Duration::ZERO).unwrap().as_secs(), 5);
        p.rejected(Duration::from_secs(5), ErrorCode::SlowDown, Some(12))
            .unwrap();
        assert_eq!(p.wait(Duration::from_secs(5)).unwrap().as_secs(), 12);
        p.rejected(
            Duration::from_secs(17),
            ErrorCode::AuthorizationPending,
            None,
        )
        .unwrap();
        assert_eq!(p.wait(Duration::from_secs(17)).unwrap().as_secs(), 12);
        assert!(p.wait(Duration::from_secs(30)).is_err());
        for code in [
            ErrorCode::AccessDenied,
            ErrorCode::ExpiredToken,
            ErrorCode::InvalidGrant,
            ErrorCode::Unauthorized,
        ] {
            assert!(p.rejected(Duration::ZERO, code, None).is_err());
        }
    }
    #[test]
    fn baseline_rules() {
        let b: BuildCommit = "a".repeat(40).parse().unwrap();
        let mut e = GitEvidence::default();
        assert_eq!(classify(Some(&b), &e), BaselineRelationship::Unknown);
        e.head = Some(b.clone());
        e.object_present = true;
        assert_eq!(classify(Some(&b), &e), BaselineRelationship::Equal);
        e.head = Some("b".repeat(40).parse().unwrap());
        for (ba, ha, expected) in [
            (Some(true), Some(false), BaselineRelationship::Ancestor),
            (Some(false), Some(true), BaselineRelationship::Descendant),
            (Some(false), Some(false), BaselineRelationship::Diverged),
            (None, None, BaselineRelationship::Unknown),
        ] {
            e.baseline_ancestor = ba;
            e.head_ancestor = ha;
            assert_eq!(classify(Some(&b), &e), expected);
        }
    }
}
