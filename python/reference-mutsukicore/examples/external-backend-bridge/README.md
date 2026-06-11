# External Backend Bridge Example

This example keeps application state outside MutsukiCore.

The "todo backend" here is only a simulated external system. MutsukiCore receives
events translated by a bridge plugin and invokes actions exposed by that bridge.
The core does not own todo data, provide CRUD storage, or act as an application
backend.

Run:

```powershell
uv run python examples/external-backend-bridge/smoke.py
```

