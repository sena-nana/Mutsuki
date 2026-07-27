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

`reference_coding_agent_test_profile()` demonstrates filesystem, Git, LSP, approval, database and
distributed selections. It is deliberately marked test-only and is never a production default.

## Product compilation

A product may compile Persona or workspace settings into instructions, prompt fragments, model
selection, tool/knowledge allowlists and context policy. The input Persona model stays outside
AgentKit. Store only the resulting profile ID and durable policy data in Agent checkpoints.

## Runtime overrides

Per-run model, budget or context overrides must remain within the product-approved profile.
Credentials are referenced by opaque Provider instance configuration and resolved by the Host
secret backend; never serialize secret values into the profile, task, checkpoint, trace or log.
