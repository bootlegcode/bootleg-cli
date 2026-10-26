//! Domain-port tests, not HTTP mocks or proof of Git/filesystem safety.
use bootleg_cli::{preview::*, preview_application::*};
use bootleg_core::{BuildCommit, ContainerId};
use std::{cell::RefCell, rc::Rc, time::Instant};

fn commit(c: char) -> BuildCommit {
    c.to_string().repeat(40).parse().unwrap()
}
fn target(c: char) -> Target {
    Target {
        container_id: ContainerId::new(),
        build_commit: Some(commit(c)),
        build_dirty: Some(false),
        connected: true,
    }
}
struct Discovery {
    pages: Vec<Vec<Target>>,
    fail: bool,
    calls: usize,
}
impl AuthorizedDiscovery for Discovery {
    fn traverse(&mut self, sink: &mut DiscoverySink, _: Instant) -> Result<(), Code> {
        self.calls += 1;
        for page in &self.pages {
            sink.begin_page()?;
            sink.response_bytes(100)?;
            for target in page {
                sink.record(target.clone())?;
            }
        }
        if self.fail {
            Err(Code::DiscoveryFailed)
        } else {
            Ok(())
        }
    }
}
#[derive(Default)]
struct Calls {
    captures: Vec<Vec<BuildCommit>>,
    comparisons: Vec<BuildCommit>,
}
struct Source {
    calls: Rc<RefCell<Calls>>,
    bad: Option<BuildCommit>,
    fail_compare: Option<BuildCommit>,
    changed: bool,
}
struct Snapshot {
    head: BuildCommit,
    calls: Rc<RefCell<Calls>>,
    bad: Option<BuildCommit>,
    fail_compare: Option<BuildCommit>,
    changed: bool,
}
impl SourceCapture for Source {
    type Snapshot = Snapshot;
    fn capture(&mut self, baselines: &[BuildCommit], _: Instant) -> Result<Snapshot, Code> {
        self.calls.borrow_mut().captures.push(baselines.to_vec());
        Ok(Snapshot {
            head: commit('b'),
            calls: self.calls.clone(),
            bad: self.bad.clone(),
            fail_compare: self.fail_compare.clone(),
            changed: self.changed,
        })
    }
}
impl CapturedSource for Snapshot {
    fn head(&self) -> Option<&BuildCommit> {
        Some(&self.head)
    }
    fn evidence(&self, baseline: &BuildCommit) -> Result<BaselineEvidence, Code> {
        Ok(BaselineEvidence {
            required_objects_present: self.bad.as_ref() != Some(baseline),
            ancestor_of_head: Some(true),
        })
    }
    fn compare(&self, baseline: &BuildCommit, _: Instant) -> Result<Vec<Change>, Code> {
        self.calls.borrow_mut().comparisons.push(baseline.clone());
        if self.fail_compare.as_ref() == Some(baseline) {
            return Err(Code::UnsupportedEntry);
        }
        if self.changed && baseline == &commit('a') {
            Ok(vec![
                compare_text(
                    b"hello",
                    None,
                    Some(TextSide {
                        mode: Mode::Regular,
                        bytes: b"SECRET CONTENT NEVER EMITTED",
                    }),
                    false,
                )?
                .unwrap(),
            ])
        } else {
            Ok(vec![])
        }
    }
}
fn source() -> Source {
    Source {
        calls: Rc::default(),
        bad: None,
        fail_compare: None,
        changed: false,
    }
}
fn discovery(pages: Vec<Vec<Target>>) -> Discovery {
    Discovery {
        pages,
        fail: false,
        calls: 0,
    }
}

#[test]
fn one_capture_and_one_comparison_per_distinct_baseline_deterministic_output() {
    let a = target('a');
    let a2 = target('a');
    let b = target('b');
    let ids = vec![
        b.container_id.clone(),
        a2.container_id.clone(),
        a.container_id.clone(),
        a.container_id.clone(),
    ];
    let mut d = discovery(vec![vec![a.clone(), b.clone()], vec![a2.clone()]]);
    let mut s = source();
    s.changed = true;
    let report = preview(&ids, &mut d, &mut s);
    assert_eq!(report.exit_code(), 1);
    assert_eq!(
        s.calls.borrow().captures,
        vec![vec![commit('a'), commit('b')]]
    );
    assert_eq!(s.calls.borrow().comparisons, vec![commit('a'), commit('b')]);
    let json = report.json().unwrap();
    assert!(!json.contains("SECRET"));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for t in value["targets"].as_array().unwrap() {
        assert_eq!(
            t["changes"].as_array().unwrap().len(),
            if t["build_commit"] == commit('a').as_str() {
                1
            } else {
                0
            }
        );
    }
    d.pages = vec![vec![b, a2, a]];
    assert_eq!(
        preview(&ids.into_iter().rev().collect::<Vec<_>>(), &mut d, &mut s)
            .json()
            .unwrap(),
        json
    );
}

#[test]
fn full_discovery_required_and_late_inconsistency_blocks_capture() {
    let t = target('a');
    let mut inconsistent = t.clone();
    inconsistent.connected = false;
    let mut d = discovery(vec![vec![t.clone()], vec![inconsistent]]);
    let mut s = source();
    let report = preview(std::slice::from_ref(&t.container_id), &mut d, &mut s);
    assert!(report.json().unwrap().contains("discovery_failed"));
    assert!(s.calls.borrow().captures.is_empty());
    d.pages = vec![vec![t.clone()]];
    d.fail = true;
    assert_eq!(preview(&[t.container_id], &mut d, &mut s).exit_code(), 2);
    assert!(s.calls.borrow().captures.is_empty());
}

#[test]
fn invalid_selection_never_discovers_and_invalid_targets_never_capture() {
    let mut d = discovery(vec![]);
    let mut s = source();
    assert_eq!(preview(&[], &mut d, &mut s).exit_code(), 2);
    assert_eq!(d.calls, 0);
    for variant in 0..4 {
        let mut t = target('a');
        match variant {
            0 => t.build_commit = None,
            1 => t.build_dirty = None,
            2 => t.build_dirty = Some(true),
            _ => t.connected = false,
        }
        d.pages = vec![vec![t.clone()]];
        assert_eq!(preview(&[t.container_id], &mut d, &mut s).exit_code(), 2);
    }
    assert!(s.calls.borrow().captures.is_empty());
}

#[test]
fn all_baselines_gated_before_comparison_and_failure_discards_partial_results() {
    let a = target('a');
    let b = target('b');
    let ids = vec![a.container_id.clone(), b.container_id.clone()];
    let mut d = discovery(vec![vec![a, b]]);
    let mut s = source();
    s.bad = Some(commit('b'));
    let report = preview(&ids, &mut d, &mut s);
    assert!(report.json().unwrap().contains("missing_object"));
    assert!(s.calls.borrow().comparisons.is_empty());
    s.bad = None;
    s.changed = true;
    s.fail_compare = Some(commit('b'));
    let json = preview(&ids, &mut d, &mut s).json().unwrap();
    assert!(json.contains("\"targets\":[]"));
    assert!(!json.contains("hello"));
    s.fail_compare = None;
    s.changed = false;
    assert_eq!(preview(&ids, &mut d, &mut s).exit_code(), 0);
}

#[test]
fn discovery_limits_are_inclusive_incremental_and_sticky() {
    let t = target('a');
    let new = || DiscoverySink::new(std::slice::from_ref(&t.container_id)).unwrap();
    let mut sink = new();
    for _ in 0..MAX_DISCOVERY_PAGES {
        sink.begin_page().unwrap();
    }
    assert_eq!(sink.begin_page(), Err(Code::LimitExceeded));
    assert_eq!(sink.response_bytes(0), Err(Code::LimitExceeded));
    let mut sink = new();
    sink.begin_page().unwrap();
    sink.response_bytes(MAX_DISCOVERY_BYTES).unwrap();
    assert_eq!(sink.response_bytes(usize::MAX), Err(Code::LimitExceeded));
    assert_eq!(sink.record(t.clone()), Err(Code::LimitExceeded));
    let mut sink = new();
    sink.begin_page().unwrap();
    let unrelated = target('b');
    for _ in 0..MAX_DISCOVERY_RECORDS {
        sink.record(unrelated.clone()).unwrap();
    }
    assert_eq!(sink.record(t.clone()), Err(Code::LimitExceeded));
    assert!(DiscoverySink::new(&[]).is_err());
    assert!(DiscoverySink::new(&vec![t.container_id; 100]).is_ok());
    assert!(
        DiscoverySink::new(
            &(0..=MAX_TARGETS)
                .map(|_| ContainerId::new())
                .collect::<Vec<_>>()
        )
        .is_err()
    );
}
