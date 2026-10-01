---
name: mutsuki-tauri-resource-bridge
description: Use when changing ResourceRef import/read/write/export, blob or preview handles, temp URL lifecycle, resource provider integration, drag/drop, clipboard, or WebView resource security.
---

# Resource Bridge Skill

Use this for `crates/mutsuki-tauri-resource` and resource-facing commands/SDK APIs.

## Boundary

- Core and plugins see `ResourceRef` descriptors.
- WebView sees safe frontend resource handles, bytes chunks, text, Blob-compatible arrays or temporary preview tokens.
- Do not expose Rust pointers, mmap handles, raw filesystem capability handles, database clients or secrets to the frontend.

## Implementation Rules

- Do not base64 large resources through Tauri invoke.
- Store imported files under the configured resource root or keep explicit external-managed descriptors.
- Temporary preview URLs/tokens must expire and be revocable.
- Read/write/export must validate the `ref_id` against the resource store before touching disk.
- Implement resource-provider methods against real stored bytes when integrating with HostRuntime.

Provider integration uses `ResourceProviderGateway::execute` (or native async execute) returning explicit result-plus-invalidation outcomes. Existing caller resource replies remain unchanged; Host actor owns lifecycle application. Invalidating providers declare Ordered. Standalone `LocalResourceClient` rejects ordered providers because it has no Core registry; use HostContext resource clients. Native async future construction must not block.
