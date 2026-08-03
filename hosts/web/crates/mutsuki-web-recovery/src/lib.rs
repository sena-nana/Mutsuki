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
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function connect() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.binaryType = 'arraybuffer';
  let sessionId = null;

  ws.addEventListener('open', () => {
    send(ws, {
      type: 'hello',
      protocol_version: '1.0.0',
      capabilities: ['recovery.read', 'recovery.write', 'host.read'],
    });
  });

  ws.addEventListener('message', async (event) => {
    const message = decodeMessage(await bytesFromMessage(event.data));
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
    send(ws, {
      type: 'rpc',
      id: crypto.randomUUID(),
      namespace: 'recovery',
      method: 'enter_safe_mode',
      params: {},
    });
  };
  document.getElementById('refresh').onclick = () => requestFailures(ws);
}

function requestFailures(ws) {
  send(ws, {
    type: 'rpc',
    id: crypto.randomUUID(),
    namespace: 'recovery',
    method: 'list_failures',
    params: {},
  });
}

function send(ws, message) {
  ws.send(encodeMessage(message));
}

async function bytesFromMessage(data) {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (data instanceof Blob) return new Uint8Array(await data.arrayBuffer());
  throw new Error('web bridge requires binary frames');
}

function encodeMessage(value) {
  const chunks = [];
  writeValue(chunks, value);
  const length = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function writeValue(chunks, value) {
  if (value === null || value === undefined) return chunks.push(Uint8Array.of(0xc0));
  if (value === false) return chunks.push(Uint8Array.of(0xc2));
  if (value === true) return chunks.push(Uint8Array.of(0xc3));
  if (typeof value === 'number') return writeNumber(chunks, value);
  if (typeof value === 'string') return writeString(chunks, value);
  if (Array.isArray(value)) {
    writeArrayHeader(chunks, value.length);
    for (const item of value) writeValue(chunks, item);
    return;
  }
  const entries = Object.entries(value).filter(([, item]) => item !== undefined);
  writeMapHeader(chunks, entries.length);
  for (const [key, item] of entries) {
    writeString(chunks, key);
    writeValue(chunks, item);
  }
}

function writeNumber(chunks, value) {
  if (Number.isInteger(value) && value >= 0 && value <= 0x7f) return chunks.push(Uint8Array.of(value));
  if (Number.isInteger(value) && value >= 0 && value <= 0xff) return chunks.push(Uint8Array.of(0xcc, value));
  if (Number.isInteger(value) && value >= 0 && value <= 0xffff) return writeHeader(chunks, 0xcd, 2, value);
  if (Number.isInteger(value) && value >= 0 && value <= 0xffffffff) return writeHeader(chunks, 0xce, 4, value);
  if (Number.isInteger(value) && value >= -32 && value < 0) return chunks.push(Uint8Array.of(0xe0 | (value + 32)));
  if (Number.isInteger(value) && value >= -0x80) return chunks.push(Uint8Array.of(0xd0, value & 0xff));
  if (Number.isInteger(value) && value >= -0x8000) return writeSigned(chunks, 0xd1, 2, value);
  if (Number.isInteger(value) && value >= -0x80000000) return writeSigned(chunks, 0xd2, 4, value);
  const out = new Uint8Array(9);
  out[0] = 0xcb;
  new DataView(out.buffer).setFloat64(1, value);
  chunks.push(out);
}

function writeString(chunks, value) {
  const bytes = textEncoder.encode(value);
  if (bytes.length <= 31) chunks.push(Uint8Array.of(0xa0 | bytes.length));
  else if (bytes.length <= 0xff) chunks.push(Uint8Array.of(0xd9, bytes.length));
  else if (bytes.length <= 0xffff) writeHeader(chunks, 0xda, 2, bytes.length);
  else writeHeader(chunks, 0xdb, 4, bytes.length);
  chunks.push(bytes);
}

function writeArrayHeader(chunks, length) {
  if (length <= 15) chunks.push(Uint8Array.of(0x90 | length));
  else if (length <= 0xffff) writeHeader(chunks, 0xdc, 2, length);
  else writeHeader(chunks, 0xdd, 4, length);
}

function writeMapHeader(chunks, length) {
  if (length <= 15) chunks.push(Uint8Array.of(0x80 | length));
  else if (length <= 0xffff) writeHeader(chunks, 0xde, 2, length);
  else writeHeader(chunks, 0xdf, 4, length);
}

function writeHeader(chunks, marker, bytes, value) {
  const out = new Uint8Array(1 + bytes);
  out[0] = marker;
  const view = new DataView(out.buffer);
  if (bytes === 2) view.setUint16(1, value);
  else view.setUint32(1, value);
  chunks.push(out);
}

function writeSigned(chunks, marker, bytes, value) {
  const out = new Uint8Array(1 + bytes);
  out[0] = marker;
  const view = new DataView(out.buffer);
  if (bytes === 2) view.setInt16(1, value);
  else view.setInt32(1, value);
  chunks.push(out);
}

function decodeMessage(bytes) {
  const reader = { bytes, offset: 0 };
  const value = readValue(reader);
  if (reader.offset !== bytes.length || !value || typeof value !== 'object') {
    throw new Error('invalid web bridge message');
  }
  return value;
}

function readValue(reader) {
  const marker = readByte(reader);
  if (marker <= 0x7f) return marker;
  if (marker >= 0xe0) return marker - 0x100;
  if ((marker & 0xe0) === 0xa0) return readString(reader, marker & 0x1f);
  if ((marker & 0xf0) === 0x90) return readArray(reader, marker & 0x0f);
  if ((marker & 0xf0) === 0x80) return readMap(reader, marker & 0x0f);
  switch (marker) {
    case 0xc0: return null;
    case 0xc2: return false;
    case 0xc3: return true;
    case 0xca: return readDataView(reader, 4).getFloat32(0);
    case 0xcb: return readDataView(reader, 8).getFloat64(0);
    case 0xcc: return readByte(reader);
    case 0xcd: return readDataView(reader, 2).getUint16(0);
    case 0xce: return readDataView(reader, 4).getUint32(0);
    case 0xcf: return Number(readDataView(reader, 8).getBigUint64(0));
    case 0xd0: return readDataView(reader, 1).getInt8(0);
    case 0xd1: return readDataView(reader, 2).getInt16(0);
    case 0xd2: return readDataView(reader, 4).getInt32(0);
    case 0xd3: return Number(readDataView(reader, 8).getBigInt64(0));
    case 0xd9: return readString(reader, readByte(reader));
    case 0xda: return readString(reader, readDataView(reader, 2).getUint16(0));
    case 0xdb: return readString(reader, readDataView(reader, 4).getUint32(0));
    case 0xdc: return readArray(reader, readDataView(reader, 2).getUint16(0));
    case 0xdd: return readArray(reader, readDataView(reader, 4).getUint32(0));
    case 0xde: return readMap(reader, readDataView(reader, 2).getUint16(0));
    case 0xdf: return readMap(reader, readDataView(reader, 4).getUint32(0));
    default: throw new Error(`unsupported MessagePack marker ${marker}`);
  }
}

function readArray(reader, length) {
  return Array.from({ length }, () => readValue(reader));
}

function readMap(reader, length) {
  const object = {};
  for (let index = 0; index < length; index += 1) {
    object[String(readValue(reader))] = readValue(reader);
  }
  return object;
}

function readString(reader, length) {
  const start = reader.offset;
  reader.offset += length;
  if (reader.offset > reader.bytes.length) throw new Error('truncated MessagePack string');
  return textDecoder.decode(reader.bytes.slice(start, reader.offset));
}

function readByte(reader) {
  if (reader.offset >= reader.bytes.length) throw new Error('truncated MessagePack frame');
  return reader.bytes[reader.offset++];
}

function readDataView(reader, length) {
  const start = reader.offset;
  reader.offset += length;
  if (reader.offset > reader.bytes.length) throw new Error('truncated MessagePack frame');
  return new DataView(reader.bytes.buffer, reader.bytes.byteOffset + start, length);
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
