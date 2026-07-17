use mutsuki_runtime_contracts::ResourceRef;
use mutsuki_tauri_bridge::{
    ApprovalResponse, FrontendError, FrontendEventEnvelope, FrontendTaskRequest,
    FrontendTaskResult, FrontendTaskRun, HostStatus, PluginSummary, PreviewHandle, ResourceBytes,
    ResourceChunk, ResourceText, RunnerSummary, TaskCancelRequest, TaskResultRequest,
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
    init_with_app(move |_| Ok(builder))
}

pub fn init_with_app<R, F>(factory: F) -> tauri::plugin::TauriPlugin<R>
where
    R: Runtime,
    F: FnOnce(&AppHandle<R>) -> Result<MutsukiTauriHostBuilder, String> + Send + 'static,
{
    tauri::plugin::Builder::new("mutsuki")
        .invoke_handler(tauri::generate_handler![
            mutsuki_call,
            mutsuki_start_task,
            mutsuki_task_result,
            mutsuki_peek_task_result,
            mutsuki_cancel_task,
            mutsuki_status,
            mutsuki_plugins_list,
            mutsuki_runners_list,
            mutsuki_resource_import_file,
            mutsuki_resource_read_bytes,
            mutsuki_resource_read_chunk,
            mutsuki_resource_read_text,
            mutsuki_resource_write_bytes,
            mutsuki_resource_export_file,
            mutsuki_resource_preview,
            mutsuki_resource_preview_release,
            mutsuki_approval_respond,
            mutsuki_approval_pending,
        ])
        .register_uri_scheme_protocol("mutsuki-resource", |context, request| {
            preview_response(context.app_handle(), request.uri())
        })
        .on_event(|app, event| {
            if matches!(event, tauri::RunEvent::Exit)
                && let Some(host) = app.try_state::<Arc<MutsukiTauriHost>>()
            {
                host.shutdown();
            }
        })
        .setup(move |app, _api| {
            let builder = factory(app.app_handle())?;
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
fn mutsuki_peek_task_result(
    host: State<'_, Arc<MutsukiTauriHost>>,
    request: TaskResultRequest,
) -> Result<FrontendTaskResult, FrontendError> {
    host.peek_task_result(request).map_err(FrontendError::from)
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
fn mutsuki_runners_list(host: State<'_, Arc<MutsukiTauriHost>>) -> Vec<RunnerSummary> {
    host.runners()
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
async fn mutsuki_resource_read_chunk(
    host: State<'_, Arc<MutsukiTauriHost>>,
    ref_id: String,
    offset: u64,
    length: usize,
) -> Result<ResourceChunk, FrontendError> {
    host.read_resource_chunk(&ref_id, offset, length)
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
fn mutsuki_resource_preview_release(
    host: State<'_, Arc<MutsukiTauriHost>>,
    token: String,
) -> Result<(), FrontendError> {
    host.release_preview_handle(&token)
        .map_err(FrontendError::from)
}

fn preview_response<R: Runtime>(
    app: &AppHandle<R>,
    uri: &tauri::http::Uri,
) -> tauri::http::Response<Vec<u8>> {
    let token = uri
        .host()
        .filter(|host| *host != "localhost" && !host.ends_with(".localhost"))
        .or_else(|| uri.path().trim_matches('/').split('/').next())
        .unwrap_or_default();
    let Some(host) = app.try_state::<Arc<MutsukiTauriHost>>() else {
        return preview_error(tauri::http::StatusCode::SERVICE_UNAVAILABLE);
    };
    match host.resource_store().read_preview_token(token) {
        Ok((bytes, media_type)) => tauri::http::Response::builder()
            .status(tauri::http::StatusCode::OK)
            .header(
                tauri::http::header::CONTENT_TYPE,
                media_type.unwrap_or_else(|| "application/octet-stream".into()),
            )
            .header(tauri::http::header::CACHE_CONTROL, "no-store")
            .header(tauri::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(bytes)
            .expect("valid preview response"),
        Err(_) => preview_error(tauri::http::StatusCode::NOT_FOUND),
    }
}

fn preview_error(status: tauri::http::StatusCode) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header(tauri::http::header::CACHE_CONTROL, "no-store")
        .body(Vec::new())
        .expect("valid preview error response")
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
