# Lifecycle details

Paths below are relative to the repository root.

Provider ABI lifecycle uses `ExecuteResourceProviderRequest` (opcode 0x300c, `resource.provider.execute`) and a `ResourceProviderResponse` containing result plus invalidations. Wire schema 1.4.0 handshake rejects older revisions; rebuild provider binaries. Native providers migrate to `execute`; no receipt-only fallback. Runner-facing resource replies retain their shapes. Python mirrors DTOs and registry/artifacts, without implementing a provider endpoint.

AsyncResourceRegistryGateway and the optional HostContext constructor argument are Rust source
changes for #182. Migrate construction sites atomically; existing provider DTOs/opcodes and wire
1.4.0 stay unchanged. A successful creation means actor registration completed, not merely that
provider storage was written. See docs/architecture/async-resource-creation.md.
