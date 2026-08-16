use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mutsuki_agent_contracts::{
    AgentError, DocumentVersion, EditorContextChangeEvent, EditorContextPolicy,
    EditorContextServiceRequest, EditorContextServiceResponse, EditorContextSnapshot,
    EditorDocumentRef, EditorWorkspaceRef, OpenDocumentSnapshot, ResourceRef, TextPosition,
    TextSelection, UnsavedBufferRef, WorkspaceEditConflict,
};
use mutsuki_runtime_contracts::{
    ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
};

const INLINE_LIMIT: usize = 2_048;

#[derive(Clone, Debug)]
struct DocumentState {
    language_id: Option<String>,
    version: DocumentVersion,
    dirty: bool,
    visible: bool,
    text: String,
    selection: Option<TextSelection>,
}

#[derive(Debug)]
struct EditorState {
    workspace: EditorWorkspaceRef,
    generation: u64,
    active: Option<EditorDocumentRef>,
    documents: BTreeMap<(String, String), DocumentState>,
    policy: EditorContextPolicy,
    supports_preview: bool,
    supports_apply: bool,
    changes: Vec<EditorContextChangeEvent>,
}

/// In-memory product-side editor context for conformance and E2E tests.
#[derive(Clone, Debug)]
pub struct FakeEditorContextService {
    inner: Arc<Mutex<EditorState>>,
}

impl Default for FakeEditorContextService {
    fn default() -> Self {
        Self::new(EditorWorkspaceRef {
            workspace_id: "ws".into(),
            folders: vec!["/workspace".into()],
            metadata: serde_json::json!({}),
        })
    }
}

impl FakeEditorContextService {
    pub fn new(workspace: EditorWorkspaceRef) -> Self {
        Self {
            inner: Arc::new(Mutex::new(EditorState {
                workspace,
                generation: 1,
                active: None,
                documents: BTreeMap::new(),
                policy: EditorContextPolicy::permissive(),
                supports_preview: true,
                supports_apply: true,
                changes: Vec::new(),
            })),
        }
    }

    pub fn with_policy(self, policy: EditorContextPolicy) -> Self {
        self.inner.lock().expect("editor mutex").policy = policy;
        self
    }

    pub fn open_document(
        &self,
        uri: impl Into<String>,
        language_id: impl Into<String>,
        text: impl Into<String>,
        active: bool,
    ) -> Result<OpenDocumentSnapshot, AgentError> {
        let uri = uri.into();
        let mut state = self.inner.lock().expect("editor mutex");
        Self::ensure_workspace_allowed(&state)?;
        let document = EditorDocumentRef {
            workspace_id: state.workspace.workspace_id.clone(),
            uri: uri.clone(),
        };
        let doc = DocumentState {
            language_id: Some(language_id.into()),
            version: DocumentVersion(1),
            dirty: false,
            visible: true,
            text: text.into(),
            selection: Some(TextSelection {
                start: TextPosition {
                    line: 0,
                    character: 0,
                },
                end: TextPosition {
                    line: 0,
                    character: 0,
                },
            }),
        };
        state
            .documents
            .insert((document.workspace_id.clone(), document.uri.clone()), doc);
        if active {
            state.active = Some(document.clone());
        }
        state.generation += 1;
        let snapshot = Self::snapshot_document(&state, &document)?;
        let generation = state.generation;
        state
            .changes
            .push(EditorContextChangeEvent::DocumentOpened {
                document: snapshot.clone(),
                generation,
            });
        Ok(snapshot)
    }

    pub fn set_selection(&self, uri: &str, selection: TextSelection) -> Result<(), AgentError> {
        let mut state = self.inner.lock().expect("editor mutex");
        if !state.policy.allow_selection {
            return Err(AgentError::new(
                "agent.editor_context.selection_denied",
                "selection capability is disabled by policy",
            ));
        }
        let document = EditorDocumentRef {
            workspace_id: state.workspace.workspace_id.clone(),
            uri: uri.into(),
        };
        let doc = state
            .documents
            .get_mut(&(document.workspace_id.clone(), document.uri.clone()))
            .ok_or_else(|| AgentError::not_found(format!("document `{uri}` is not open")))?;
        doc.selection = Some(selection);
        state.generation += 1;
        let generation = state.generation;
        state
            .changes
            .push(EditorContextChangeEvent::ActiveSelectionChanged {
                document,
                selection,
                generation,
            });
        Ok(())
    }

    pub fn edit_unsaved(
        &self,
        uri: &str,
        text: impl Into<String>,
    ) -> Result<DocumentVersion, AgentError> {
        let mut state = self.inner.lock().expect("editor mutex");
        if !state.policy.allow_unsaved_buffers {
            return Err(AgentError::new(
                "agent.editor_context.buffer_denied",
                "unsaved buffer capability is disabled by policy",
            ));
        }
        let document = EditorDocumentRef {
            workspace_id: state.workspace.workspace_id.clone(),
            uri: uri.into(),
        };
        let doc = state
            .documents
            .get_mut(&(document.workspace_id.clone(), document.uri.clone()))
            .ok_or_else(|| AgentError::not_found(format!("document `{uri}` is not open")))?;
        doc.text = text.into();
        doc.dirty = true;
        doc.version = DocumentVersion(doc.version.0 + 1);
        let version = doc.version;
        let doc_snapshot = doc.clone();
        let buffer = Self::buffer_for(&state, &document, &doc_snapshot);
        state.generation += 1;
        let generation = state.generation;
        state
            .changes
            .push(EditorContextChangeEvent::DocumentChanged {
                document,
                version,
                generation,
                buffer: Some(buffer),
            });
        Ok(version)
    }

    pub fn freeze_snapshot(
        &self,
        turn_id: Option<String>,
    ) -> Result<EditorContextSnapshot, AgentError> {
        let state = self.inner.lock().expect("editor mutex");
        Self::ensure_workspace_allowed(&state)?;
        Ok(Self::build_snapshot(&state, turn_id))
    }

    pub fn assert_edit_base(
        &self,
        document: &EditorDocumentRef,
        expected_version: DocumentVersion,
    ) -> Result<(), WorkspaceEditConflict> {
        let state = self.inner.lock().expect("editor mutex");
        let actual = state
            .documents
            .get(&(document.workspace_id.clone(), document.uri.clone()))
            .map(|doc| doc.version)
            .unwrap_or(DocumentVersion(0));
        if actual != expected_version {
            return Err(WorkspaceEditConflict::stale(
                document.clone(),
                expected_version,
                actual,
            ));
        }
        Ok(())
    }

    pub fn call(
        &self,
        request: EditorContextServiceRequest,
    ) -> Result<EditorContextServiceResponse, AgentError> {
        let state = self.inner.lock().expect("editor mutex");
        Self::ensure_workspace_allowed(&state)?;
        Ok(match request {
            EditorContextServiceRequest::GetWorkspace => EditorContextServiceResponse::Workspace {
                workspace: state.workspace.clone(),
                generation: state.generation,
            },
            EditorContextServiceRequest::GetActiveDocument => {
                let document = state
                    .active
                    .as_ref()
                    .map(|document| Self::snapshot_document(&state, document))
                    .transpose()?;
                EditorContextServiceResponse::ActiveDocument {
                    document,
                    generation: state.generation,
                }
            }
            EditorContextServiceRequest::ListOpenDocuments { visible_only } => {
                let mut documents = Vec::new();
                for key in state.documents.keys() {
                    let document = EditorDocumentRef {
                        workspace_id: key.0.clone(),
                        uri: key.1.clone(),
                    };
                    let snapshot = Self::snapshot_document(&state, &document)?;
                    if !visible_only || snapshot.visible {
                        documents.push(snapshot);
                    }
                }
                EditorContextServiceResponse::OpenDocuments {
                    documents,
                    generation: state.generation,
                }
            }
            EditorContextServiceRequest::ReadUnsavedBuffer {
                document,
                expected_version,
            } => {
                if !state.policy.allow_unsaved_buffers {
                    return Err(AgentError::new(
                        "agent.editor_context.buffer_denied",
                        "unsaved buffer capability is disabled by policy",
                    ));
                }
                let doc = state
                    .documents
                    .get(&(document.workspace_id.clone(), document.uri.clone()))
                    .ok_or_else(|| {
                        AgentError::not_found(format!("document `{}` is not open", document.uri))
                    })?;
                if doc.version != expected_version {
                    return Err(AgentError::new(
                        "agent.editor_context.stale_version",
                        WorkspaceEditConflict::stale(
                            document.clone(),
                            expected_version,
                            doc.version,
                        )
                        .message,
                    ));
                }
                EditorContextServiceResponse::UnsavedBuffer {
                    buffer: Self::buffer_for(&state, &document, doc),
                    generation: state.generation,
                }
            }
            EditorContextServiceRequest::Snapshot { turn_id } => {
                EditorContextServiceResponse::Snapshot {
                    snapshot: Self::build_snapshot(&state, turn_id),
                }
            }
            EditorContextServiceRequest::Capabilities => {
                EditorContextServiceResponse::Capabilities {
                    supports_workspace_edit_preview: state.supports_preview,
                    supports_workspace_edit_apply: state.supports_apply,
                    generation: state.generation,
                }
            }
        })
    }

    pub fn change_log(&self) -> Vec<EditorContextChangeEvent> {
        self.inner.lock().expect("editor mutex").changes.clone()
    }

    fn ensure_workspace_allowed(state: &EditorState) -> Result<(), AgentError> {
        if state.policy.allowed_workspace_ids.is_empty() {
            return Ok(());
        }
        if state
            .policy
            .allowed_workspace_ids
            .iter()
            .any(|id| id == &state.workspace.workspace_id)
        {
            Ok(())
        } else {
            Err(AgentError::new(
                "agent.editor_context.workspace_denied",
                format!(
                    "workspace `{}` is outside the allowed profile scope",
                    state.workspace.workspace_id
                ),
            ))
        }
    }

    fn build_snapshot(state: &EditorState, turn_id: Option<String>) -> EditorContextSnapshot {
        let documents = state
            .documents
            .keys()
            .filter_map(|key| {
                let document = EditorDocumentRef {
                    workspace_id: key.0.clone(),
                    uri: key.1.clone(),
                };
                Self::snapshot_document(state, &document).ok()
            })
            .collect::<Vec<_>>();
        EditorContextSnapshot {
            snapshot_id: format!(
                "snap:{}:{}:{}",
                state.workspace.workspace_id,
                state.generation,
                turn_id.unwrap_or_else(|| "anon".into())
            ),
            workspace: state.workspace.clone(),
            generation: state.generation,
            active_document: state.active.clone(),
            documents,
            supports_workspace_edit_preview: state.supports_preview,
            supports_workspace_edit_apply: state.supports_apply,
        }
    }

    fn snapshot_document(
        state: &EditorState,
        document: &EditorDocumentRef,
    ) -> Result<OpenDocumentSnapshot, AgentError> {
        let doc = state
            .documents
            .get(&(document.workspace_id.clone(), document.uri.clone()))
            .ok_or_else(|| {
                AgentError::not_found(format!("document `{}` is not open", document.uri))
            })?;
        Ok(OpenDocumentSnapshot {
            document: document.clone(),
            version: doc.version,
            language_id: doc.language_id.clone(),
            dirty: doc.dirty,
            visible: doc.visible,
            active: state.active.as_ref() == Some(document),
            selection: if state.policy.allow_selection {
                doc.selection
            } else {
                None
            },
            buffer: if state.policy.allow_unsaved_buffers && doc.dirty {
                Some(Self::buffer_for(state, document, doc))
            } else {
                None
            },
        })
    }

    fn buffer_for(
        state: &EditorState,
        document: &EditorDocumentRef,
        doc: &DocumentState,
    ) -> UnsavedBufferRef {
        let bytes = doc.text.as_bytes();
        let inline_limit = if state.policy.max_inline_buffer_bytes == 0 {
            INLINE_LIMIT
        } else {
            state.policy.max_inline_buffer_bytes as usize
        };
        if bytes.len() <= inline_limit {
            return UnsavedBufferRef {
                document: document.clone(),
                version: doc.version,
                content_ref: None,
                inline_text: Some(doc.text.clone()),
                byte_length: bytes.len() as u64,
            };
        }
        let slot = format!(
            "{}:{}:{}",
            document.workspace_id, document.uri, doc.version.0
        );
        UnsavedBufferRef {
            document: document.clone(),
            version: doc.version,
            content_ref: Some(ResourceRef {
                ref_id: format!("editor-buffer:{slot}").into(),
                resource_id: ResourceId {
                    kind_id: "mutsuki.agent.editor.buffer".into(),
                    slot_id: slot,
                    generation: 1,
                    version: doc.version.0,
                },
                semantic: ResourceSemantic::VersionedSnapshot,
                provider_id: "mutsuki.agent.testkit.fake_editor".into(),
                resource_kind: "mutsuki.agent.editor.buffer".into(),
                schema: "mutsuki.agent.editor.buffer@1".into(),
                version: doc.version.0,
                generation: 1,
                access: ResourceAccess::Blob {
                    store_id: "editor-fake".into(),
                    key: format!("{}@{}", document.uri, doc.version.0),
                },
                size_hint: Some(bytes.len() as u64),
                content_hash: None,
                lifetime: ResourceLifetime::BorrowedUntilTaskEnd,
                lease: None,
                seal_state: ResourceSealState::Sealed,
            }),
            inline_text: None,
            byte_length: bytes.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_editor_covers_active_selection_unsaved_buffer_and_stale_edit() {
        let editor = FakeEditorContextService::default();
        let opened = editor
            .open_document("file:///workspace/main.rs", "rust", "fn main() {}", true)
            .unwrap();
        assert!(opened.active);
        editor
            .set_selection(
                "file:///workspace/main.rs",
                TextSelection {
                    start: TextPosition {
                        line: 0,
                        character: 3,
                    },
                    end: TextPosition {
                        line: 0,
                        character: 7,
                    },
                },
            )
            .unwrap();
        let version = editor
            .edit_unsaved(
                "file:///workspace/main.rs",
                "fn main() { println!(\"hi\"); }",
            )
            .unwrap();
        let snapshot = editor.freeze_snapshot(Some("turn-1".into())).unwrap();
        assert_eq!(
            snapshot.active_document.as_ref().unwrap().uri,
            opened.document.uri
        );
        assert_eq!(snapshot.generation, 4);
        let after_ui_edit = editor
            .edit_unsaved(
                "file:///workspace/main.rs",
                "fn main() { println!(\"later\"); }",
            )
            .unwrap();
        assert_ne!(after_ui_edit, snapshot.documents[0].version);
        assert_eq!(snapshot.documents[0].version, version);
        let stale = editor
            .assert_edit_base(&opened.document, DocumentVersion(version.0))
            .unwrap_err();
        assert_eq!(stale.expected_version, version);
        assert_eq!(stale.actual_version, after_ui_edit);

        let large = "x".repeat((64 * 1024) + 32);
        editor
            .open_document("file:///workspace/big.txt", "text", "", false)
            .unwrap();
        editor
            .edit_unsaved("file:///workspace/big.txt", large)
            .unwrap();
        let buffer = match editor
            .call(EditorContextServiceRequest::ReadUnsavedBuffer {
                document: EditorDocumentRef {
                    workspace_id: "ws".into(),
                    uri: "file:///workspace/big.txt".into(),
                },
                expected_version: DocumentVersion(2),
            })
            .unwrap()
        {
            EditorContextServiceResponse::UnsavedBuffer { buffer, .. } => buffer,
            other => panic!("unexpected response: {other:?}"),
        };
        assert!(buffer.content_ref.is_some());
        assert!(buffer.inline_text.is_none());
    }

    #[test]
    fn performance_smoke_snapshot_loop() {
        let editor = FakeEditorContextService::default();
        for index in 0..64 {
            editor
                .open_document(
                    format!("file:///workspace/f{index}.rs"),
                    "rust",
                    format!("// file {index}"),
                    index == 0,
                )
                .unwrap();
        }
        let started = std::time::Instant::now();
        for _ in 0..200 {
            let _ = editor.freeze_snapshot(Some("turn".into())).unwrap();
        }
        assert!(
            started.elapsed().as_millis() < 500,
            "editor snapshot smoke exceeded budget"
        );
    }
}
