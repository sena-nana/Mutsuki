#![no_main]

//! Agent wire envelopes cross a Link connection, so both ends decode bytes they did not produce.
//! The request enum is internally tagged and the response carries a `Result`, shapes where a
//! decode that succeeds but re-encodes differently would silently desynchronize the two peers.

use libfuzzer_sys::fuzz_target;
use mutsuki_agent_contracts::{AgentWireRequestEnvelope, AgentWireResponseEnvelope};

fuzz_target!(|data: &[u8]| {
    if let Ok(request) = serde_json::from_slice::<AgentWireRequestEnvelope>(data) {
        let encoded = serde_json::to_vec(&request).expect("request envelope serializes");
        let decoded: AgentWireRequestEnvelope =
            serde_json::from_slice(&encoded).expect("request envelope round trips");
        assert_eq!(decoded, request);
    }

    if let Ok(response) = serde_json::from_slice::<AgentWireResponseEnvelope>(data) {
        let encoded = serde_json::to_vec(&response).expect("response envelope serializes");
        let decoded: AgentWireResponseEnvelope =
            serde_json::from_slice(&encoded).expect("response envelope round trips");
        assert_eq!(decoded, response);
    }
});
