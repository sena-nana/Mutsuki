use mutsuki_runtime_contracts::ResourceRef;
use mutsuki_tauri_bridge::{
    ApprovalResponse, FrontendError, FrontendEventEnvelope, FrontendTaskRequest,
    FrontendTaskResult, FrontendTaskRun, HostStatus, PluginSummary, PreviewHandle, ResourceBytes,
    ResourceText, TaskCancelRequest, TaskResultRequest,
};
use mutsuki_tauri_host::{MutsukiTauriHost, MutsukiTauriHostBuilder};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    init_with_host(MutsukiTauriHost::builder())
}

pub fn init_with_host<R: Runtime>(
    builder: MutsukiTauriHostBuilder,
) -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("mutsuki")
        .invoke_handler(tauri::generate_handler![
            mutsuki_call,
            mutsuki_start_task,
            mutsuki_task_result,
            mutsuki_cancel_task,
            mutsuki_status,
            mutsuki_plugins_list,
            mutsuki_resource_import_file,
            mutsuki_resource_read_bytes,
            mutsuki_resource_read_text,
            mutsuki_resource_write_bytes,
            mutsuki_resource_export_file,
            mutsuki_resource_preview,
            mutsuki_approval_respond,
            mutsuki_approval_pending,
        ])
        .setup(move |app, _api| {
            let host = Arc::new(builder.build().map_err(|error| error.to_string())?);
            forward_events(app.app_handle().clone(), host.clone());
            app.manage(host);
            Ok(())
        })
        .build()
}

fn forward_events<R: Runtime>(app: AppHandle<R>, host: Arc<MutsukiTauriHost>) {
    let mut rx = host.event_hub().subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(envelope) = rx.recv().await {
            let _ = app.emit("mutsuki://event", envelope.clone());
            let channel = envelope.channel.clone();
            let _ = app.emit(&channel, envelope);
        }
    });
}

#[tauri::command]
fn mutsuki_call(
    host: State<'_, Arc<MutsukiTauriHost>>,
    request: FrontendTaskRequest,
) -> Result<FrontendTaskResult, FrontendError> {
    host.call(request).map_err(FrontendError::from)
}

#[tauri::command]
fn mutsuki_start_task(
    host: State<'_, Arc<MutsukiTauriHost>>,
    request: FrontendTaskRequest,
) -> Result<FrontendTaskRun, FrontendError> {
    host.start_task(request).map_err(FrontendError::from)
}

#[tauri::command]
fn mutsuki_task_result(
    host: State<'_, Arc<MutsukiTauriHost>>,
    request: TaskResultRequest,
) -> Result<FrontendTaskResult, FrontendError> {
    host.task_result(request).map_err(FrontendError::from)
}

#[tauri::command]
fn mutsuki_cancel_task(
    host: State<'_, Arc<MutsukiTauriHost>>,
    request: TaskCancelRequest,
) -> Result<String, FrontendError> {
    host.cancel_task(request).map_err(FrontendError::from)
}

#[tauri::command]
fn mutsuki_status(host: State<'_, Arc<MutsukiTauriHost>>) -> HostStatus {
    host.status()
}

#[tauri::command]
fn mutsuki_plugins_list(host: State<'_, Arc<MutsukiTauriHost>>) -> Vec<PluginSummary> {
    host.plugins()
}

#[tauri::command]
async fn mutsuki_resource_import_file(
    host: State<'_, Arc<MutsukiTauriHost>>,
    path: String,
) -> Result<ResourceRef, FrontendError> {
    host.import_file(path).await.map_err(FrontendError::from)
}

#[tauri::command]
async fn mutsuki_resource_read_bytes(
    host: State<'_, Arc<MutsukiTauriHost>>,
    ref_id: String,
) -> Result<ResourceBytes, FrontendError> {
    host.read_resource_bytes(&ref_id)
        .await
        .map_err(FrontendError::from)
}

#[tauri::command]
async fn mutsuki_resource_read_text(
    host: State<'_, Arc<MutsukiTauriHost>>,
    ref_id: String,
) -> Result<ResourceText, FrontendError> {
    host.read_resource_text(&ref_id)
        .await
        .map_err(FrontendError::from)
}

#[tauri::command]
async fn mutsuki_resource_write_bytes(
    host: State<'_, Arc<MutsukiTauriHost>>,
    ref_id: String,
    bytes: Vec<u8>,
) -> Result<ResourceRef, FrontendError> {
    host.write_resource_bytes(&ref_id, bytes)
        .await
        .map_err(FrontendError::from)
}

#[tauri::command]
async fn mutsuki_resource_export_file(
    host: State<'_, Arc<MutsukiTauriHost>>,
    ref_id: String,
    target: String,
) -> Result<(), FrontendError> {
    host.export_resource_to_file(&ref_id, target)
        .await
        .map_err(FrontendError::from)
}

#[tauri::command]
fn mutsuki_resource_preview(
    host: State<'_, Arc<MutsukiTauriHost>>,
    ref_id: String,
) -> Result<PreviewHandle, FrontendError> {
    host.create_preview_handle(&ref_id)
        .map_err(FrontendError::from)
}

#[tauri::command]
fn mutsuki_approval_respond(
    host: State<'_, Arc<MutsukiTauriHost>>,
    response: ApprovalResponse,
) -> Result<String, FrontendError> {
    host.resolve_approval(response)
        .map(|decision| format!("{decision:?}").to_lowercase())
        .map_err(FrontendError::from)
}

#[tauri::command]
fn mutsuki_approval_pending(
    host: State<'_, Arc<MutsukiTauriHost>>,
) -> Vec<mutsuki_tauri_bridge::ApprovalRequest> {
    host.pending_approvals()
}

#[allow(dead_code)]
fn _assert_event_serializable(_event: FrontendEventEnvelope) {}
