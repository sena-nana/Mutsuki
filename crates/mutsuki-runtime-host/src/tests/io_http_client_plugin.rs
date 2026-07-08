use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use mutsuki_plugin_io_http_client::{HTTP_REQUEST_PROTOCOL, PLUGIN_ID};
use mutsuki_runtime_contracts::{RuntimeProfile, RuntimeProfileMode, Task, TaskStatus};
use mutsuki_runtime_sdk::BuiltinPluginLoader;
use serde_json::json;

use crate::{HostRuntimeCommand, HostRuntimeReply, RuntimeBootstrapper};

#[test]
fn io_http_client_plugin_executes_allowlisted_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\nmutsuki")
            .unwrap();
    });

    let mut host = RuntimeBootstrapper::new();
    let mut loader =
        BuiltinPluginLoader::new().with_plugin(Box::new(mutsuki_plugin_io_http_client::plugin()));
    host.load_plugins(&mut loader).unwrap();
    let runtime = host.into_host_runtime(http_profile()).unwrap();

    runtime
        .dispatch(HostRuntimeCommand::SubmitTask(Box::new(Task::new(
            "http-request",
            HTTP_REQUEST_PROTOCOL,
            json!({
                "method": "GET",
                "url": format!("http://127.0.0.1:{port}/hello"),
                "domain_allowlist": ["127.0.0.1"],
                "timeout_ms": 2000,
            }),
        ))))
        .unwrap();

    let reply = runtime
        .dispatch(HostRuntimeCommand::RunUntilIdle { max_ticks: 8 })
        .unwrap();
    let HostRuntimeReply::Idle(_report) = reply else {
        panic!("expected idle reply");
    };

    assert_eq!(
        runtime.task_status("http-request"),
        Some(TaskStatus::Completed)
    );
    assert_eq!(
        runtime.task_status("http-request:effect"),
        Some(TaskStatus::Completed)
    );
    server.join().unwrap();
}

fn http_profile() -> RuntimeProfile {
    RuntimeProfile {
        profile_id: "io-http-client".into(),
        mode: RuntimeProfileMode::FullDev,
        enabled_plugins: vec![PLUGIN_ID.into()],
        bindings: Default::default(),
        plugin_deployments: Default::default(),
        allow_dynamic_registration: false,
        allow_hot_reload: true,
    }
}
