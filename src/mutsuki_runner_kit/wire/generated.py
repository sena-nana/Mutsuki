"""Generated from MutsukiCore runtime-wire schema at revision 1d423251.

Do not add operation names outside this module. `test_wire_schema.py` verifies
this generated registry against the checked-in Core artifact.
"""

from enum import IntEnum

CORE_WIRE_REVISION = "1d42325107a82f98dda3912097c3c0aefd4907ba"


class Opcode(IntEnum):
    PLUGIN_INITIALIZE = 0x0001
    RUNNER_RUN_BATCH = 0x1001
    RUNNER_CANCEL = 0x1002
    RUNNER_DISPOSE = 0x1003
    TASK_SUBMIT_BATCH = 0x2001
    TASK_CANCEL = 0x2002
    TASK_OUTCOME = 0x2003
    RESOURCE_READ_COLLECT = 0x3001
    RESOURCE_READ_SNAPSHOT = 0x3002
    RESOURCE_STREAM_OPEN = 0x3003
    RESOURCE_EXPORT = 0x3004
    RESOURCE_WRITE_COMMIT = 0x3005
    RESOURCE_COMMAND = 0x3006
    RESOURCE_COMMAND_BATCH = 0x3007
    RESOURCE_SAGA = 0x3008
    RESOURCE_CREATE_BLOB = 0x3009
    RESOURCE_CREATE_COW_STATE = 0x300A
    RESOURCE_CREATE_CAPABILITY = 0x300B


OPCODE_METHODS: dict[Opcode, str] = {
    Opcode.PLUGIN_INITIALIZE: "plugin.initialize",
    Opcode.RUNNER_RUN_BATCH: "runner.run_batch",
    Opcode.RUNNER_CANCEL: "runner.cancel",
    Opcode.RUNNER_DISPOSE: "runner.dispose",
    Opcode.TASK_SUBMIT_BATCH: "task.submit_batch",
    Opcode.TASK_CANCEL: "task.cancel",
    Opcode.TASK_OUTCOME: "task.outcome",
    Opcode.RESOURCE_READ_COLLECT: "resource.read.collect",
    Opcode.RESOURCE_READ_SNAPSHOT: "resource.read.snapshot",
    Opcode.RESOURCE_STREAM_OPEN: "resource.stream.open",
    Opcode.RESOURCE_EXPORT: "resource.export",
    Opcode.RESOURCE_WRITE_COMMIT: "resource.write.commit",
    Opcode.RESOURCE_COMMAND: "resource.command",
    Opcode.RESOURCE_COMMAND_BATCH: "resource.command_batch",
    Opcode.RESOURCE_SAGA: "resource.saga",
    Opcode.RESOURCE_CREATE_BLOB: "resource.create_blob",
    Opcode.RESOURCE_CREATE_COW_STATE: "resource.create_cow_state",
    Opcode.RESOURCE_CREATE_CAPABILITY: "resource.create_capability",
}

MANAGEMENT_OPCODES = frozenset(
    {
        Opcode.PLUGIN_INITIALIZE,
        Opcode.RUNNER_CANCEL,
        Opcode.RUNNER_DISPOSE,
    }
)
