//! Retention benchmark for app-delivery receipt/operation history.
//!
//! Verifies that 100_000 unique request IDs keep store occupancy at the
//! configured ceiling, and that eviction does not introduce multi-millisecond
//! pauses on the hot path.

use std::time::{Duration, Instant};

use mutsuki_runtime_contracts::{
    CapabilityDescriptor, DeliveryReceipt, IdempotentReceiptStore, ReceiptRetentionPolicy,
};
use mutsuki_tauri_host::{
    AppDeliveryOptions, AppDeliveryService, AppId, AppIdentity, DeliveryDraftStore,
    InMemoryAppLinkTransport, OperationHistoryPolicy, ProcessAppActivator,
};
use serde_json::json;

#[tokio::main]
async fn main() {
    let total = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000_usize);
    let max_entries = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000_usize);

    let policy = ReceiptRetentionPolicy {
        max_entries: Some(max_entries),
        max_bytes: Some(16 * 1024 * 1024),
        ttl: None,
    };
    let mut store = IdempotentReceiptStore::with_policy(policy);

    let target = AppId::new("bench.target").unwrap();
    let transport = InMemoryAppLinkTransport::new();
    transport.register_online(
        &target,
        vec![CapabilityDescriptor::new("bench.capability", 1, 1)],
    );
    let service = AppDeliveryService::with_operation_policy(
        AppIdentity {
            app_id: AppId::new("bench.source").unwrap(),
            instance_id: "bench-1".into(),
        },
        ProcessAppActivator::new(),
        transport,
        DeliveryDraftStore::memory(),
        OperationHistoryPolicy {
            max_terminal_entries: max_entries,
        },
    );

    let start_rss = current_rss_bytes();
    let started = Instant::now();
    let mut max_insert_ns = 0_u128;
    let mut samples = Vec::with_capacity(64);

    for index in 0..total {
        let request_id = format!("req-{index}");
        let insert_started = Instant::now();
        let receipt = DeliveryReceipt::Completed {
            request_id: request_id.clone(),
            remote_task_id: Some(format!("task-{index}")),
            output: json!({"payload": "x".repeat(64)}),
        };
        let recorded = store.accept_or_duplicate(request_id.clone(), receipt);
        assert!(matches!(recorded, DeliveryReceipt::Completed { .. }));

        let delivered = service
            .request_app(
                target.clone(),
                CapabilityDescriptor::new("bench.capability", 1, 1),
                json!({"n": index}),
                AppDeliveryOptions {
                    request_id: Some(request_id),
                    activate_if_offline: false,
                    ready_timeout: Duration::from_secs(1),
                    request_timeout: Duration::from_secs(1),
                    persist_on_failure: false,
                },
            )
            .await
            .expect("deliver");
        assert!(matches!(
            delivered,
            DeliveryReceipt::Accepted { .. } | DeliveryReceipt::Duplicate { .. }
        ));

        let insert_ns = insert_started.elapsed().as_nanos();
        max_insert_ns = max_insert_ns.max(insert_ns);
        if index % (total / 16).max(1) == 0 {
            samples.push(insert_ns);
        }
    }

    let elapsed = started.elapsed();
    let end_rss = current_rss_bytes();
    let stats = store.stats();
    let op_stats = service.operation_stats();

    let newest = format!("req-{}", total - 1);
    assert!(matches!(
        store.accept_or_duplicate(
            newest.clone(),
            DeliveryReceipt::Accepted {
                request_id: newest,
                remote_task_id: Some("ignored".into()),
            }
        ),
        DeliveryReceipt::Duplicate { .. }
    ));

    let oldest = "req-0".to_string();
    assert!(matches!(
        store.accept_or_duplicate(
            oldest.clone(),
            DeliveryReceipt::Accepted {
                request_id: oldest,
                remote_task_id: Some("fresh".into()),
            }
        ),
        DeliveryReceipt::Accepted { .. }
    ));

    println!(
        "bounded_after total={} max_entries={} store_entries={} store_bytes={} store_evictions={} op_entries={} op_terminal={} op_evictions={} elapsed_ms={} max_insert_us={} p50_sample_us={} start_rss={} end_rss={} delta_rss={}",
        total,
        max_entries,
        stats.entries,
        stats.estimated_bytes,
        stats.evictions,
        op_stats.entries,
        op_stats.terminal_entries,
        op_stats.evictions,
        elapsed.as_millis(),
        max_insert_ns / 1_000,
        percentile_us(&samples, 0.50),
        start_rss,
        end_rss,
        end_rss.saturating_sub(start_rss),
    );

    assert!(
        stats.entries <= max_entries + 1,
        "receipt entries exceeded budget: {}",
        stats.entries
    );
    assert!(
        op_stats.terminal_entries <= max_entries + 1,
        "operation terminal entries exceeded budget: {}",
        op_stats.terminal_entries
    );
    assert!(
        stats.evictions >= (total.saturating_sub(max_entries)) as u64,
        "expected evictions for overflow"
    );
    assert!(
        max_insert_ns < Duration::from_millis(50).as_nanos(),
        "eviction pause too large: {} ns",
        max_insert_ns
    );
}

fn percentile_us(samples: &[u128], q: f64) -> u128 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[index] / 1_000
}

fn current_rss_bytes() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}
