# AgentRuntimeProfile assembly

`AgentRuntimeProfile` is a serializable product assembly contract. AgentKit provides the DTO,
builder, validation and a test-only reference coding profile; it does not load product configuration.

The profile composes:

- runtime mode and turn/iteration policy;
- protocol Adapter selection and Provider instance descriptors;
- enabled Plugin IDs, tools and services;
- system instructions and ordered prompt fragments;
- context and permission policy;
- token/cost budget;
- persistence and distribution service references.

Use `AgentRuntimeProfileBuilder`, add every selected surface explicitly, then call `build`.
Validation rejects empty/duplicate IDs, Adapter-to-Provider mismatches, overlapping permission
allow/deny entries, invalid limits, missing durable/distributed service references, and test-only
Providers in production mode.

`reference_coding_agent_test_profile()` demonstrates a Native Coding Agent assembly: two OpenAI-
compatible Provider instances on one protocol Adapter, Computer Use, Git, LSP, Code Index, Next
Edit, MCP, Skills/Knowledge, approval, compaction, SubAgent-capable persistence, and distributed
selections. It is deliberately marked test-only and is never a production default.

`NativeCodingAgentBundle` (`mutsuki-agent-bundle`) wires those shared services as single-instance
`Arc` handles for Agent tools and product UI. Hosts inject process/LSP/MCP backends and credentials;
the bundle never starts Claude Code / Codex official Agent Server processes.

## Product compilation

A product may compile Persona or workspace settings into instructions, prompt fragments, model
selection, tool/knowledge allowlists and context policy. The input Persona model stays outside
AgentKit. Editor buffers, Project/Task UI, and chat-platform sessions likewise stay in product
repositories; AgentKit only consumes neutral contracts such as editor context snapshots, coding
wire events, and shared services (for example `GitService` and `CodeCompletionService`). Store only
the resulting profile ID and durable policy data in Agent checkpoints.

Inline code completion uses `CodeCompletionService` with a protocol Model Adapter directly: no Agent
session, tool loop, or official Agent Server. Products (VS Code Extension, LiliaCode, other IDEs)
share the same request/response contract and apply local debounce, generation fencing, and
document-version rejection at the editor edge.

## Runtime overrides

Per-run model, budget or context overrides must remain within the product-approved profile.
Provider instances store only `CredentialRef`. Secret material is owned by the Host
secret/keyring boundary and resolved through `CredentialBrokerService` short-lived handles.
Model Adapters never read keyring files, browser cookies, or third-party CLI private storage
directly. Never serialize secret values into the profile, task, checkpoint, trace, event or log.
