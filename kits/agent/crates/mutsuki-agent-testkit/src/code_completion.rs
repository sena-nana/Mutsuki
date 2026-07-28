use std::sync::Arc;

use mutsuki_agent_contracts::{DocumentVersion, EditorDocumentRef, TextPosition};
use mutsuki_agent_plugin_code_completion::{
    CodeCompletionConfig, CodeCompletionService, DeterministicCompletionAdapter,
    request_from_snapshot, test_provider,
};

use crate::FakeEditorContextService;

/// Fake-editor + deterministic adapter completion E2E shared by products/tests.
pub fn complete_from_fake_editor(
    editor: &FakeEditorContextService,
    uri: &str,
    generation: u64,
    prefix: &str,
    suffix: &str,
) -> mutsuki_agent_contracts::CodeCompletionResponse {
    let snapshot = editor
        .freeze_snapshot(None)
        .expect("editor snapshot for completion");
    let document = snapshot
        .active_document
        .clone()
        .unwrap_or(EditorDocumentRef {
            workspace_id: snapshot.workspace.workspace_id.clone(),
            uri: uri.into(),
        });
    let version = snapshot
        .documents
        .iter()
        .find(|doc| doc.document.uri == document.uri)
        .map(|doc| doc.version)
        .unwrap_or(DocumentVersion(1));
    let language_id = snapshot
        .documents
        .iter()
        .find(|doc| doc.document.uri == document.uri)
        .and_then(|doc| doc.language_id.clone());
    let service = CodeCompletionService::new(
        Arc::new(DeterministicCompletionAdapter::new()),
        test_provider("memory://deterministic"),
        CodeCompletionConfig::default(),
    );
    service
        .complete(request_from_snapshot(
            format!("editor-{generation}"),
            generation,
            snapshot.workspace.workspace_id.clone(),
            document,
            language_id,
            version,
            TextPosition {
                line: 0,
                character: prefix.len() as u32,
            },
            prefix,
            suffix,
        ))
        .expect("completion")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_agent_contracts::{
        CodeCompletionFeedback, CodeCompletionFeedbackAction, CodeCompletionStatus,
    };
    use mutsuki_agent_plugin_code_completion::hash_text;

    #[test]
    fn fake_editor_and_deterministic_adapter_complete_e2e() {
        let editor = FakeEditorContextService::default();
        editor
            .open_document("file:///workspace/main.rs", "rust", "fn main() {", true)
            .unwrap();
        let response =
            complete_from_fake_editor(&editor, "file:///workspace/main.rs", 1, "fn main() {", "\n");
        assert_eq!(response.status, CodeCompletionStatus::Ready);
        assert_eq!(response.candidates[0].insert_text, " println!(\"hi\"); }");
        assert!(response.may_display(DocumentVersion(1), 1));

        let bumped = editor
            .edit_unsaved("file:///workspace/main.rs", "fn main() { // typed")
            .unwrap();
        assert!(response.is_stale(bumped));
        assert!(!response.may_display(bumped, 1));
    }

    #[test]
    fn feedback_path_reuses_service_contract_hashes() {
        let editor = FakeEditorContextService::default();
        editor
            .open_document("file:///workspace/main.rs", "rust", "let x =", true)
            .unwrap();
        let response =
            complete_from_fake_editor(&editor, "file:///workspace/main.rs", 1, "let x =", "");
        let service = CodeCompletionService::new(
            Arc::new(DeterministicCompletionAdapter::new()),
            test_provider("memory://deterministic"),
            CodeCompletionConfig::default(),
        );
        service
            .record_feedback(CodeCompletionFeedback {
                request_id: response.request_id.clone(),
                generation: response.generation,
                candidate_id: response.candidates.first().map(|c| c.candidate_id.clone()),
                action: CodeCompletionFeedbackAction::Ignore,
                document: EditorDocumentRef {
                    workspace_id: "ws".into(),
                    uri: "file:///workspace/main.rs".into(),
                },
                document_version: DocumentVersion(1),
                language_id: Some("rust".into()),
                prefix_hash: hash_text("let x ="),
                suffix_hash: hash_text(""),
            })
            .unwrap();
        assert_eq!(service.feedback_len(), 1);
    }
}
