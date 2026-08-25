#![no_main]

//! Flow documents arrive from the Web Console as operator-authored JSON and are validated before
//! they are ever activated. The validator walks node/edge graphs that the document itself shapes,
//! so a malformed graph — dangling edges, duplicate ports, self-references — must be reported as
//! issues rather than panicking the config apply path.

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use mutsuki_bot_flow::{BotNodeCatalog, validate_flow};
use mutsuki_bot_protocol::BotFlowDocument;
use mutsuki_plugin_bot_command::bot_command_manifest;

static CATALOG: LazyLock<BotNodeCatalog> = LazyLock::new(|| {
    BotNodeCatalog::from_manifests(&[bot_command_manifest(1)]).expect("command node catalog")
});

fuzz_target!(|data: &[u8]| {
    let Ok(flow) = serde_json::from_slice::<BotFlowDocument>(data) else {
        return;
    };
    let result = validate_flow(&flow, &CATALOG);
    // `valid` and the issue list are what the Console renders and what activation gates on; they
    // must never disagree.
    assert_eq!(
        result.valid,
        !result
            .issues
            .iter()
            .any(|issue| issue.severity == mutsuki_bot_protocol::BotFlowValidationSeverity::Error)
    );

    // A document that survived decoding has to survive a round trip, otherwise a stored flow could
    // fail to reload after a restart.
    let encoded = serde_json::to_vec(&flow).expect("flow document serializes");
    let decoded: BotFlowDocument = serde_json::from_slice(&encoded).expect("flow document reloads");
    assert_eq!(decoded, flow);
});
