use std::cell::RefCell;

use super::fixtures::*;
use crate::*;
use mutsukicore_runtime_contracts::*;

#[test]
fn runtime_selects_primary_candidate_by_priority_then_id() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-b", 1)).unwrap();
    runtime.register_agent(agent("agent-a", 1)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();
    runtime.start_agent("agent-b", &mut backend).unwrap();

    assert_eq!(
        runtime.select_accepting(&envelope()),
        Some("agent-a".into())
    );
}

#[test]
fn runtime_select_accepting_ignores_unregistered_source() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    let mut unknown = envelope();
    unknown.source.source_id = "source:unknown".into();

    assert_eq!(runtime.select_accepting(&unknown), None);
}

#[test]
fn custom_election_policy_only_sees_prefiltered_candidates() {
    struct PreferB<'a> {
        seen: &'a RefCell<Vec<ElectionCandidate>>,
    }

    impl ElectionPolicy for PreferB<'_> {
        fn select(&self, candidates: &[ElectionCandidate]) -> Option<AgentId> {
            self.seen.borrow_mut().extend_from_slice(candidates);
            candidates
                .iter()
                .find(|candidate| candidate.agent_id == "agent-b")
                .map(|candidate| candidate.agent_id.clone())
                .or_else(|| Some("sleeping-agent".into()))
        }
    }

    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 10)).unwrap();
    runtime.register_agent(agent("agent-b", 1)).unwrap();

    let mut observer = agent("observer-agent", 99);
    observer.participation = AgentParticipation::Observer;
    runtime.register_agent(observer).unwrap();

    let mut helper = agent("helper-agent", 98);
    helper.participation = AgentParticipation::ExplicitHelper;
    runtime.register_agent(helper).unwrap();

    let mut empty_accepts = agent("empty-accepts-agent", 97);
    empty_accepts.accepts.clear();
    runtime.register_agent(empty_accepts).unwrap();

    let mut no_match = agent("no-match-agent", 96);
    no_match.accepts = vec![ScopeRuleSpec::BySchemaPrefix {
        prefix: "other.".into(),
    }];
    runtime.register_agent(no_match).unwrap();

    runtime.register_agent(agent("sleeping-agent", 95)).unwrap();
    runtime.register_agent(agent("stopped-agent", 94)).unwrap();

    for agent_id in [
        "agent-a",
        "agent-b",
        "observer-agent",
        "helper-agent",
        "empty-accepts-agent",
        "no-match-agent",
        "stopped-agent",
    ] {
        runtime.start_agent(agent_id, &mut backend).unwrap();
    }
    runtime.stop_agent("stopped-agent", &mut backend).unwrap();

    let seen = RefCell::new(Vec::new());
    let policy = PreferB { seen: &seen };

    assert_eq!(
        runtime.select_accepting(&envelope()),
        Some("agent-a".into())
    );
    assert_eq!(
        runtime.select_accepting_with_policy(&envelope(), &policy),
        Some("agent-b".into())
    );

    let mut seen_snapshot = seen.borrow().clone();
    seen_snapshot.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    assert_eq!(
        seen_snapshot,
        vec![
            ElectionCandidate {
                agent_id: "agent-a".into(),
                priority: 10
            },
            ElectionCandidate {
                agent_id: "agent-b".into(),
                priority: 1
            }
        ]
    );

    runtime.stop_agent("agent-b", &mut backend).unwrap();
    assert_eq!(
        runtime.select_accepting_with_policy(&envelope(), &policy),
        None
    );
}

#[test]
fn election_policy_is_not_called_when_prefiltered_candidates_are_empty() {
    struct ShouldNotRun;

    impl ElectionPolicy for ShouldNotRun {
        fn select(&self, _candidates: &[ElectionCandidate]) -> Option<AgentId> {
            panic!("policy must not run without prefiltered candidates");
        }
    }

    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    let mut agent = agent("agent-a", 0);
    agent.accepts.clear();
    runtime.register_agent(agent).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();

    assert_eq!(
        runtime.select_accepting_with_policy(&envelope(), &ShouldNotRun),
        None
    );
}
