use mutsuki_agent_contracts::*;
use mutsuki_runtime_sdk::{ProtocolSpec, SdkProtocol};

macro_rules! protocol_marker {
    ($name:ident, $id:expr) => {
        #[derive(Clone, Debug)]
        pub struct $name;

        impl SdkProtocol for $name {
            const PROTOCOL_ID: &'static str = $id;
        }

        impl ProtocolSpec for $name {}
    };
}

protocol_marker!(AgentRunProtocol, AGENT_RUN_PROTOCOL);
protocol_marker!(AgentContextBuildProtocol, AGENT_CONTEXT_BUILD_PROTOCOL);
protocol_marker!(AgentToolListProtocol, AGENT_TOOL_LIST_PROTOCOL);
protocol_marker!(AgentToolExecuteProtocol, AGENT_TOOL_EXECUTE_PROTOCOL);
protocol_marker!(AgentSessionCreateProtocol, AGENT_SESSION_CREATE_PROTOCOL);
protocol_marker!(AgentSessionGetProtocol, AGENT_SESSION_GET_PROTOCOL);
protocol_marker!(AgentSessionAppendProtocol, AGENT_SESSION_APPEND_PROTOCOL);
protocol_marker!(
    AgentSessionSnapshotProtocol,
    AGENT_SESSION_SNAPSHOT_PROTOCOL
);
protocol_marker!(AgentSessionForkProtocol, AGENT_SESSION_FORK_PROTOCOL);
protocol_marker!(AgentMemoryQueryProtocol, AGENT_MEMORY_QUERY_PROTOCOL);
protocol_marker!(AgentMemoryWriteProtocol, AGENT_MEMORY_WRITE_PROTOCOL);
protocol_marker!(AgentMemoryActivateProtocol, AGENT_MEMORY_ACTIVATE_PROTOCOL);
protocol_marker!(AgentMemoryDisableProtocol, AGENT_MEMORY_DISABLE_PROTOCOL);
protocol_marker!(AgentMemoryDeleteProtocol, AGENT_MEMORY_DELETE_PROTOCOL);
protocol_marker!(AgentMemoryReviseProtocol, AGENT_MEMORY_REVISE_PROTOCOL);
protocol_marker!(
    AgentMemoryOpportunityProtocol,
    AGENT_MEMORY_OPPORTUNITY_PROTOCOL
);
protocol_marker!(AgentMemoryFeedbackProtocol, AGENT_MEMORY_FEEDBACK_PROTOCOL);
protocol_marker!(AgentModelGenerateProtocol, AGENT_MODEL_GENERATE_PROTOCOL);
protocol_marker!(AgentModelStreamProtocol, AGENT_MODEL_STREAM_PROTOCOL);
protocol_marker!(
    AgentModelHttpEffectProtocol,
    AGENT_MODEL_HTTP_EFFECT_PROTOCOL
);
protocol_marker!(AgentModelPollProtocol, AGENT_MODEL_POLL_PROTOCOL);
protocol_marker!(AgentPromptRenderProtocol, AGENT_PROMPT_RENDER_PROTOCOL);
protocol_marker!(AgentPromptGetProtocol, AGENT_PROMPT_GET_PROTOCOL);
protocol_marker!(AgentTranscribeProtocol, AGENT_TRANSCRIBE_PROTOCOL);
protocol_marker!(
    AgentSpeechSynthesizeProtocol,
    AGENT_SPEECH_SYNTHESIZE_PROTOCOL
);
protocol_marker!(AgentScheduleCreateProtocol, AGENT_SCHEDULE_CREATE_PROTOCOL);
protocol_marker!(AgentScheduleListProtocol, AGENT_SCHEDULE_LIST_PROTOCOL);
protocol_marker!(AgentScheduleGetProtocol, AGENT_SCHEDULE_GET_PROTOCOL);
protocol_marker!(AgentScheduleUpdateProtocol, AGENT_SCHEDULE_UPDATE_PROTOCOL);
protocol_marker!(AgentSchedulePauseProtocol, AGENT_SCHEDULE_PAUSE_PROTOCOL);
protocol_marker!(AgentScheduleResumeProtocol, AGENT_SCHEDULE_RESUME_PROTOCOL);
protocol_marker!(AgentScheduleCancelProtocol, AGENT_SCHEDULE_CANCEL_PROTOCOL);
protocol_marker!(
    AgentScheduleHistoryProtocol,
    AGENT_SCHEDULE_HISTORY_PROTOCOL
);
protocol_marker!(AgentScheduleDueProtocol, AGENT_SCHEDULE_DUE_PROTOCOL);
protocol_marker!(AgentSkillDiscoverProtocol, AGENT_SKILL_DISCOVER_PROTOCOL);
protocol_marker!(AgentSkillLoadProtocol, AGENT_SKILL_LOAD_PROTOCOL);
protocol_marker!(AgentSkillReloadProtocol, AGENT_SKILL_RELOAD_PROTOCOL);
protocol_marker!(
    AgentKnowledgeIngestProtocol,
    AGENT_KNOWLEDGE_INGEST_PROTOCOL
);
protocol_marker!(
    AgentKnowledgeRetrieveProtocol,
    AGENT_KNOWLEDGE_RETRIEVE_PROTOCOL
);
protocol_marker!(
    AgentKnowledgeDeleteProtocol,
    AGENT_KNOWLEDGE_DELETE_PROTOCOL
);
protocol_marker!(AgentEditorContextProtocol, AGENT_EDITOR_CONTEXT_PROTOCOL);
protocol_marker!(AgentCodeIndexProtocol, AGENT_CODE_INDEX_PROTOCOL);
protocol_marker!(AgentCodeCompletionProtocol, AGENT_CODE_COMPLETION_PROTOCOL);
protocol_marker!(AgentNextEditProtocol, AGENT_NEXT_EDIT_PROTOCOL);
protocol_marker!(AgentGitProtocol, AGENT_GIT_PROTOCOL);
protocol_marker!(
    AgentCredentialLoginProtocol,
    AGENT_CREDENTIAL_LOGIN_PROTOCOL
);
protocol_marker!(
    AgentCredentialImportProtocol,
    AGENT_CREDENTIAL_IMPORT_PROTOCOL
);
protocol_marker!(
    AgentCredentialRefreshProtocol,
    AGENT_CREDENTIAL_REFRESH_PROTOCOL
);
protocol_marker!(
    AgentCredentialRevokeProtocol,
    AGENT_CREDENTIAL_REVOKE_PROTOCOL
);
protocol_marker!(
    AgentCredentialStatusProtocol,
    AGENT_CREDENTIAL_STATUS_PROTOCOL
);
protocol_marker!(
    AgentCredentialIssueHandleProtocol,
    AGENT_CREDENTIAL_ISSUE_HANDLE_PROTOCOL
);
