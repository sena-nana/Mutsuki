use super::fixtures::*;
use crate::*;
use mutsuki_runtime_contracts::*;

#[test]
fn resource_gate_tracks_descriptor_leases_without_handles() {
    let mut gate = ResourceGate::new();
    let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");

    let lease = gate.acquire(&ref_id, "agent-a").unwrap();
    assert_eq!(lease.ref_id, "ref-1");
    assert_eq!(lease.token_id, "lease-00000000000000000000000001");
    assert_eq!(gate.list_records()[0].lease_count, 1);

    gate.release(&lease).unwrap();
    assert_eq!(gate.list_records()[0].lease_count, 0);
}

#[test]
fn resource_backend_filters_records_by_owner() {
    let mut gate = ResourceGate::new();
    gate.register(ref_descriptor("ref-a", "domain.resource"), "owner-a");
    gate.register(ref_descriptor("ref-b", "domain.resource"), "owner-b");

    let owner_a = <ResourceGate as ResourceBackend>::list_records(&gate, Some("owner-a"));
    assert_eq!(owner_a.len(), 1);
    assert_eq!(owner_a[0].descriptor.ref_id, "ref-a");
}

#[test]
fn resource_gate_rejects_forged_lease_token_without_releasing() {
    let mut gate = ResourceGate::new();
    let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");

    let lease = gate.acquire(&ref_id, "agent-a").unwrap();
    let forged = LeaseToken {
        token_id: lease.token_id.clone(),
        ref_id: "ref-other".into(),
        owner: "agent-b".into(),
    };

    let err = gate.release(&forged).unwrap_err();
    assert_eq!(err.error().code, "ref.not_found");
    assert_eq!(
        err.error().evidence.get("reason"),
        Some(&ScalarValue::String("lease_token_mismatch".into()))
    );
    assert_eq!(gate.list_records()[0].lease_count, 1);

    gate.release(&lease).unwrap();
    assert_eq!(gate.list_records()[0].lease_count, 0);
}

#[test]
fn standalone_resource_gate_does_not_collect_event_drafts() {
    let mut gate = ResourceGate::new();
    let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "resource-host");
    let lease = gate.acquire(&ref_id, "agent-a").unwrap();
    gate.release(&lease).unwrap();

    assert!(gate.event_drafts().is_empty());
}

#[test]
fn runtime_assigns_global_event_sequence_to_pending_resource_events() {
    let mut runtime = AgentRuntime::new();
    let mut backend = backend();
    runtime.register_agent(agent("agent-a", 0)).unwrap();
    runtime.start_agent("agent-a", &mut backend).unwrap();
    let ref_id = runtime
        .resources_mut()
        .register(ref_descriptor("ref-1", "domain.resource"), "resource-host");
    let lease = runtime.resources_mut().acquire(&ref_id, "agent-a").unwrap();

    let snapshot = runtime.events();
    assert!(
        snapshot
            .iter()
            .any(|event| event.name == "resource.register")
    );
    assert!(
        snapshot
            .iter()
            .any(|event| event.name == "resource.acquire")
    );
    assert!(
        snapshot
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );

    let drained = runtime.drain_events();
    assert_eq!(drained, snapshot);
    assert!(runtime.events().is_empty());

    runtime.resources_mut().release(&lease).unwrap();
    runtime.publish(envelope()).unwrap();
    let events = runtime.events();
    let release_index = events
        .iter()
        .position(|event| event.name == "resource.release")
        .unwrap();
    let publish_index = events
        .iter()
        .position(|event| event.name == "runtime.publish")
        .unwrap();
    assert!(release_index < publish_index);
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
}

#[test]
fn runtime_emits_structured_resource_error_events() {
    let mut runtime = AgentRuntime::new();

    let acquire_err = runtime
        .resources_mut()
        .acquire("ref-missing", "agent-a")
        .unwrap_err();
    assert_eq!(acquire_err.error().code, "ref.not_found");
    let events = runtime.events();
    let event = events
        .iter()
        .find(|event| event.name == "resource.acquire.error")
        .unwrap();
    assert_eq!(event.kind, RuntimeEventKind::Resource);
    assert_eq!(event.error.as_ref().unwrap().code, "ref.not_found");
    assert_eq!(
        event.attributes.get("ref_id"),
        Some(&ScalarValue::String("ref-missing".into()))
    );

    runtime.drain_events();
    let stale = LeaseToken {
        token_id: "lease-missing".into(),
        ref_id: "ref-missing".into(),
        owner: "agent-a".into(),
    };
    let release_err = runtime.resources_mut().release(&stale).unwrap_err();
    assert_eq!(release_err.error().code, "ref.not_found");
    let events = runtime.events();
    let event = events
        .iter()
        .find(|event| event.name == "resource.release.error")
        .unwrap();
    assert_eq!(event.kind, RuntimeEventKind::Resource);
    assert_eq!(event.error.as_ref().unwrap().code, "ref.not_found");
    assert_eq!(
        event.attributes.get("token_id"),
        Some(&ScalarValue::String("lease-missing".into()))
    );
}

#[test]
fn resource_gate_enforces_ref_quota_without_incrementing_leases() {
    let mut policy = ResourceQuotaPolicy::default();
    policy.max_leases_by_ref.insert("ref-1".into(), 1);
    let mut gate = ResourceGate::with_quota_policy(policy);
    let ref_id = gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");

    let lease = gate.acquire(&ref_id, "agent-a").unwrap();
    let err = gate.acquire(&ref_id, "agent-b").unwrap_err();
    assert_eq!(err.error().code, ERR_CAPABILITY_EXHAUSTED);
    assert_eq!(
        err.error().evidence.get("dimension"),
        Some(&ScalarValue::String("ref_id".into()))
    );
    assert_eq!(gate.list_records()[0].lease_count, 1);

    gate.release(&lease).unwrap();
}

#[test]
fn resource_gate_enforces_kind_quota_and_ref_quota_takes_precedence() {
    let mut policy = ResourceQuotaPolicy::default();
    policy
        .max_leases_by_kind
        .insert("domain.resource".into(), 1);
    let mut gate = ResourceGate::with_quota_policy(policy.clone());
    gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");
    gate.register(ref_descriptor("ref-2", "domain.resource"), "backend");
    gate.acquire("ref-1", "agent-a").unwrap();
    let kind_err = gate.acquire("ref-2", "agent-b").unwrap_err();
    assert_eq!(
        kind_err.error().evidence.get("dimension"),
        Some(&ScalarValue::String("kind".into()))
    );
    assert_eq!(
        kind_err.error().evidence.get("current"),
        Some(&ScalarValue::Int(1))
    );
    let records = gate.list_records();
    assert_eq!(
        records.iter().map(|record| record.lease_count).sum::<u64>(),
        1
    );

    policy.max_leases_by_ref.insert("ref-1".into(), 1);
    let mut gate = ResourceGate::with_quota_policy(policy);
    gate.register(ref_descriptor("ref-1", "domain.resource"), "backend");
    gate.acquire("ref-1", "agent-a").unwrap();
    let ref_err = gate.acquire("ref-1", "agent-b").unwrap_err();
    assert_eq!(
        ref_err.error().evidence.get("dimension"),
        Some(&ScalarValue::String("ref_id".into()))
    );
}
