//! Read-only preview use case. Ports carry no credentials, labels, or runtime DTOs.
//! Concrete adapters must enforce the v1 no-write and capture safety contracts;
//! these interfaces alone are not evidence of filesystem/process safety.
use crate::preview::{self, Change, Code, Diagnostic, Report, Target, TargetPreview};
use bootleg_core::{BuildCommit, ContainerId};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub const MAX_DISCOVERY_PAGES: usize = 100;
pub const MAX_DISCOVERY_RECORDS: usize = 20_000;
pub const MAX_DISCOVERY_BYTES: usize = 16 * 1024 * 1024;
pub const INVOCATION_TIMEOUT: Duration = Duration::from_secs(120);

/// Incremental discovery sink. Charge response bytes *before* buffering/decoding
/// each chunk, and records before retaining them. All records count, but only
/// requested metadata is retained. Any limit violation permanently poisons it.
pub struct DiscoverySink {
    requested: BTreeMap<String, ContainerId>,
    selected: Vec<Target>,
    pages: usize,
    records: usize,
    bytes: usize,
    failure: Option<Code>,
}
impl DiscoverySink {
    pub fn new(requested: &[ContainerId]) -> Result<Self, Code> {
        let mut ids = BTreeMap::new();
        for id in requested {
            ids.insert(id.to_string(), id.clone());
            if ids.len() > preview::MAX_TARGETS {
                return Err(Code::LimitExceeded);
            }
        }
        if ids.is_empty() {
            return Err(Code::LimitExceeded);
        }
        Ok(Self {
            requested: ids,
            selected: vec![],
            pages: 0,
            records: 0,
            bytes: 0,
            failure: None,
        })
    }
    fn check(&mut self, valid: bool) -> Result<(), Code> {
        if !valid && self.failure.is_none() {
            self.failure = Some(Code::LimitExceeded);
        }
        self.failure.map_or(Ok(()), Err)
    }
    /// Call before requesting each page, including empty pages.
    pub fn begin_page(&mut self) -> Result<(), Code> {
        self.check(self.pages < MAX_DISCOVERY_PAGES)?;
        self.pages += 1;
        Ok(())
    }
    /// Count decoded response-body bytes (not compressed transport bytes).
    pub fn response_bytes(&mut self, bytes: usize) -> Result<(), Code> {
        self.check(self.pages > 0 && bytes <= MAX_DISCOVERY_BYTES - self.bytes)?;
        self.bytes += bytes;
        Ok(())
    }
    pub fn record(&mut self, target: Target) -> Result<(), Code> {
        self.check(self.pages > 0 && self.records < MAX_DISCOVERY_RECORDS)?;
        self.records += 1;
        if self
            .requested
            .contains_key(&target.container_id.to_string())
        {
            self.selected.push(target);
        }
        Ok(())
    }
    fn finish(self) -> Result<Vec<preview::AcceptedTarget>, Vec<Diagnostic>> {
        if let Some(code) = self.failure {
            return Err(vec![Diagnostic::new(code, None)]);
        }
        if self.pages == 0 {
            return Err(vec![Diagnostic::new(Code::DiscoveryFailed, None)]);
        }
        preview::select_targets(
            &self.requested.into_values().collect::<Vec<_>>(),
            &self.selected,
        )
    }
}

pub trait AuthorizedDiscovery {
    /// Traverse every authorized page, even after all requested IDs are found.
    /// Return success only at the end of pagination. Bound parsing allocations,
    /// charge the sink incrementally, and honor the shared absolute deadline.
    /// Only existing bootstrap/discovery GETs are permitted.
    fn traverse(&mut self, sink: &mut DiscoverySink, deadline: Instant) -> Result<(), Code>;
}

pub struct BaselineEvidence {
    pub required_objects_present: bool,
    pub ancestor_of_head: Option<bool>,
}

pub trait CapturedSource {
    /// Evidence and comparisons must all refer to this same immutable capture.
    fn head(&self) -> Option<&BuildCommit>;
    fn evidence(&self, baseline: &BuildCommit) -> Result<BaselineEvidence, Code>;
    /// Return bounded metadata, never file contents. Implementations apply the
    /// domain comparison policy and stop before exceeding MAX_CHANGES.
    fn compare(&self, baseline: &BuildCommit, deadline: Instant) -> Result<Vec<Change>, Code>;
}

pub trait SourceCapture {
    type Snapshot: CapturedSource;
    /// Capture once for the sorted distinct baselines. Enforce shared input/path
    /// budgets, pre-read sensitive gates, no-follow reads, repository/index gates,
    /// and before/after race checks before returning. No lazy source rereads in
    /// Snapshot. Git operations have a 30s cap within the shared deadline.
    fn capture(
        &mut self,
        baselines: &[BuildCommit],
        deadline: Instant,
    ) -> Result<Self::Snapshot, Code>;
}

fn failure(code: Code) -> Report {
    Report::failure(vec![Diagnostic::new(code, None)])
}
fn check_deadline(deadline: Instant) -> Result<(), Code> {
    if Instant::now() >= deadline {
        Err(Code::LimitExceeded)
    } else {
        Ok(())
    }
}

/// All-or-nothing orchestration. No renderer sees partial targets. Adapters must
/// also enforce deadlines while blocked; between-call checks cannot interrupt I/O.
pub fn preview(
    requested: &[ContainerId],
    discovery: &mut impl AuthorizedDiscovery,
    source: &mut impl SourceCapture,
) -> Report {
    run(
        requested,
        discovery,
        source,
        Instant::now() + INVOCATION_TIMEOUT,
    )
}
fn run(
    requested: &[ContainerId],
    discovery: &mut impl AuthorizedDiscovery,
    source: &mut impl SourceCapture,
    deadline: Instant,
) -> Report {
    let mut sink = match DiscoverySink::new(requested) {
        Ok(sink) => sink,
        Err(code) => return failure(code),
    };
    if let Err(code) =
        check_deadline(deadline).and_then(|()| discovery.traverse(&mut sink, deadline))
    {
        return failure(code);
    }
    let targets = match sink.finish() {
        Ok(targets) => targets,
        Err(errors) => return Report::failure(errors),
    };
    let baselines: Vec<_> = targets
        .iter()
        .map(|t| (t.build_commit.as_str(), t.build_commit.clone()))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    let snapshot =
        match check_deadline(deadline).and_then(|()| source.capture(&baselines, deadline)) {
            Ok(snapshot) => snapshot,
            Err(code) => return failure(code),
        };
    // Gate every baseline before computing any comparison.
    let mut relationships = BTreeMap::new();
    let mut errors = Vec::new();
    for baseline in &baselines {
        let result = check_deadline(deadline)
            .and_then(|()| snapshot.evidence(baseline))
            .and_then(|e| {
                preview::relationship(
                    baseline,
                    snapshot.head(),
                    e.required_objects_present,
                    e.ancestor_of_head,
                )
            });
        match result {
            Ok(relationship) => {
                relationships.insert(baseline.as_str(), relationship);
            }
            Err(code) => errors.extend(
                targets
                    .iter()
                    .filter(|t| t.build_commit == *baseline)
                    .map(|t| Diagnostic::new(code, Some(t.container_id.clone()))),
            ),
        }
    }
    if !errors.is_empty() {
        return Report::failure(errors);
    }
    let mut comparisons = BTreeMap::new();
    for baseline in &baselines {
        match check_deadline(deadline).and_then(|()| snapshot.compare(baseline, deadline)) {
            Ok(changes) if changes.len() <= preview::MAX_CHANGES => {
                comparisons.insert(baseline.as_str(), changes);
            }
            result => {
                let code = result.err().unwrap_or(Code::LimitExceeded);
                return Report::failure(
                    targets
                        .iter()
                        .filter(|t| t.build_commit == *baseline)
                        .map(|t| Diagnostic::new(code, Some(t.container_id.clone())))
                        .collect(),
                );
            }
        }
    }
    let mut output = Vec::new();
    for target in targets {
        let baseline = target.build_commit.as_str();
        let relationship = relationships[baseline];
        let changes = comparisons[baseline].clone();
        match TargetPreview::new(target, relationship, changes) {
            Ok(target) => output.push(target),
            Err(code) => return failure(code),
        }
    }
    let report = match Report::success(output) {
        Ok(report) => report,
        Err(code) => return failure(code),
    };
    // Validate the final bounded projection before it can reach either renderer.
    if let Err(code) = report.json().and_then(|_| check_deadline(deadline)) {
        return failure(code);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_deadlines_fail_closed() {
        assert_eq!(check_deadline(Instant::now()), Err(Code::LimitExceeded));
        assert_eq!(check_deadline(Instant::now() + INVOCATION_TIMEOUT), Ok(()));
    }

    #[test]
    fn ignored_sink_errors_and_empty_traversals_cannot_succeed() {
        let id = ContainerId::new();
        let mut sink = DiscoverySink::new(std::slice::from_ref(&id)).unwrap();
        sink.begin_page().unwrap();
        let _ = sink.response_bytes(MAX_DISCOVERY_BYTES + 1);
        assert_eq!(
            sink.finish(),
            Err(vec![Diagnostic::new(Code::LimitExceeded, None)])
        );
        let sink = DiscoverySink::new(&[id]).unwrap();
        assert_eq!(
            sink.finish(),
            Err(vec![Diagnostic::new(Code::DiscoveryFailed, None)])
        );
    }
}
