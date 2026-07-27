//! Minimal Recovery Shell assets and status model.
//!
//! Recovery is intentionally small: connection/start status, extension failures,
//! disable/crash isolation, cache clear, safe mode, and structured errors.
//! It must not grow into a full plugin management console.

use std::path::{Path, PathBuf};

use mutsuki_web_protocol::{ExtensionFailure, WebHostStatusReport, WebShellAssets};
use serde::{Deserialize, Serialize};

/// Snapshot exposed to the recovery shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySnapshot {
    pub host: WebHostStatusReport,
    pub failures: Vec<ExtensionFailure>,
    pub safe_mode: bool,
    pub disabled_extensions: Vec<String>,
}

/// Built-in recovery shell served when business extensions fail.
#[derive(Debug, Clone)]
pub struct RecoveryShell {
    root_dir: PathBuf,
}

impl RecoveryShell {
    pub fn from_embedded_dir(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    /// Materialize the minimal recovery shell into `target_dir`.
    pub fn materialize(&self, target_dir: &Path) -> std::io::Result<WebShellAssets> {
        std::fs::create_dir_all(target_dir)?;
        let index = target_dir.join("index.html");
        std::fs::write(&index, RECOVERY_INDEX_HTML)?;
        let app_js = target_dir.join("recovery.js");
        std::fs::write(&app_js, RECOVERY_JS)?;
        Ok(WebShellAssets {
            root_dir: target_dir.to_path_buf(),
            index_file: "index.html".into(),
            import_map: serde_json::Map::new(),
        })
    }

    pub fn assets(&self) -> WebShellAssets {
        WebShellAssets {
            root_dir: self.root_dir.clone(),
            index_file: "index.html".into(),
            import_map: Default::default(),
        }
    }
}

const RECOVERY_INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta http-equiv="Content-Security-Policy"
        content="default-src 'self'; connect-src 'self' ws: wss:; script-src 'self'; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'" />
  <title>Mutsuki Recovery Shell</title>
  <style>
    body { font-family: ui-sans-serif, system-ui, sans-serif; margin: 2rem; color: #102018; background: #f4f7f5; }
    h1 { font-size: 1.4rem; }
    .card { background: #fff; border: 1px solid #d5ddd8; padding: 1rem 1.25rem; margin: 1rem 0; }
    button { margin-right: .5rem; }
    code { background: #e8efea; padding: .1rem .3rem; }
  </style>
</head>
<body>
  <h1>Mutsuki Recovery Shell</h1>
  <p>Minimal recovery only. Business plugin management lives outside WebHost.</p>
  <div class="card">
    <div id="status">Connecting…</div>
    <div id="failures"></div>
    <button id="safe-mode">Enter safe mode</button>
    <button id="refresh">Refresh</button>
  </div>
  <script type="module" src="./recovery.js"></script>
</body>
</html>
"#;

const RECOVERY_JS: &str = r#"
const statusEl = document.getElementById('status');
const failuresEl = document.getElementById('failures');

function connect() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  let sessionId = null;

  ws.addEventListener('open', () => {
    ws.send(JSON.stringify({
      type: 'hello',
      protocol_version: '1.0.0',
      capabilities: ['recovery.read', 'recovery.write', 'host.read'],
    }));
  });

  ws.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.type === 'hello_ack') {
      sessionId = message.session.session_id;
      statusEl.textContent = `Connected. safe_mode=${message.session.safe_mode}`;
      requestFailures(ws);
      return;
    }
    if (message.type === 'rpc_result') {
      if (message.error) {
        failuresEl.textContent = message.error.message;
        return;
      }
      const failures = message.result || [];
      failuresEl.innerHTML = failures.length
        ? `<ul>${failures.map((f) => `<li><code>${f.extension_id}</code>: ${f.message}</li>`).join('')}</ul>`
        : '<p>No extension failures.</p>';
    }
  });

  document.getElementById('safe-mode').onclick = () => {
    ws.send(JSON.stringify({
      type: 'rpc',
      id: crypto.randomUUID(),
      namespace: 'recovery',
      method: 'enter_safe_mode',
      params: {},
    }));
  };
  document.getElementById('refresh').onclick = () => requestFailures(ws);
}

function requestFailures(ws) {
  ws.send(JSON.stringify({
    type: 'rpc',
    id: crypto.randomUUID(),
    namespace: 'recovery',
    method: 'list_failures',
    params: {},
  }));
}

connect();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn materialize_writes_recovery_shell() {
        let dir = tempdir().unwrap();
        let shell = RecoveryShell::from_embedded_dir(dir.path());
        let assets = shell.materialize(dir.path()).unwrap();
        assert!(assets.root_dir.join("index.html").is_file());
        assert!(assets.root_dir.join("recovery.js").is_file());
        let html = std::fs::read_to_string(assets.root_dir.join("index.html")).unwrap();
        assert!(html.contains("Recovery Shell"));
        assert!(!html.contains("plugin marketplace"));
    }
}
