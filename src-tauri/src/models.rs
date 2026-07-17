use serde::Serialize;

use crate::workspace_file_kind::{ContentMode, WorkspaceFileKind};

#[derive(Debug, Serialize, Clone)]
pub(crate) struct MarkdownFileEntry {
    pub(crate) kind: WorkspaceFileKind,
    pub(crate) path: String,
    pub(crate) relative_path: String,
    pub(crate) name: String,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct WorkspaceDirectoryEntry {
    pub(crate) path: String,
    pub(crate) relative_path: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct OpenFileResponse {
    pub(crate) kind: WorkspaceFileKind,
    pub(crate) path: String,
    pub(crate) content_mode: ContentMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bytes_base64: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PreparedOpenFileResponse {
    pub(crate) file: OpenFileResponse,
    pub(crate) open_receipt: String,
    pub(crate) commit_operation_id: String,
}

pub(crate) const ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum ActiveDocumentDiskSnapshot {
    Present {
        file: OpenFileResponse,
        preview_revision: u64,
    },
    Missing {
        path: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActiveDocumentWatchReason {
    Changed,
    Renamed,
    Resync,
    Missing,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ActiveDocumentWatchRegistration {
    pub(crate) protocol_version: u8,
    pub(crate) watch_id: String,
    pub(crate) document_id: String,
    pub(crate) document_generation: u64,
    pub(crate) sequence: u64,
    pub(crate) snapshot: ActiveDocumentDiskSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ActiveDocumentWatchSnapshotEnvelope {
    pub(crate) protocol_version: u8,
    pub(crate) watch_id: String,
    pub(crate) document_id: String,
    pub(crate) document_generation: u64,
    pub(crate) sequence: u64,
    pub(crate) reason: ActiveDocumentWatchReason,
    pub(crate) previous_path: Option<String>,
    pub(crate) snapshot: ActiveDocumentDiskSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActiveDocumentWatchHealthStatus {
    Degraded,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ActiveDocumentWatchEventPayload {
    State {
        reason: ActiveDocumentWatchReason,
        previous_path: Option<String>,
        snapshot: ActiveDocumentDiskSnapshot,
    },
    Health {
        status: ActiveDocumentWatchHealthStatus,
        message: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ActiveDocumentWatchEvent {
    pub(crate) protocol_version: u8,
    pub(crate) watch_id: String,
    pub(crate) document_id: String,
    pub(crate) document_generation: u64,
    pub(crate) sequence: u64,
    pub(crate) event: ActiveDocumentWatchEventPayload,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RecentFileSummary {
    pub(crate) id: String,
    pub(crate) display_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RecentFilesSnapshot {
    pub(crate) entries: Vec<RecentFileSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum OpenCommitResult {
    Committed { recent_files: RecentFilesSnapshot },
    NotCommitted { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum OpenCommitStatus {
    Pending,
    Committed { recent_files: RecentFilesSnapshot },
    NotCommitted { message: String },
    Unknown,
}

#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceDirectoryListing {
    pub(crate) root: String,
    pub(crate) files: Vec<MarkdownFileEntry>,
    pub(crate) directories: Vec<WorkspaceDirectoryEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceSnapshot {
    pub(crate) workspace_token: String,
    pub(crate) root: String,
    pub(crate) files: Vec<MarkdownFileEntry>,
    pub(crate) directories: Vec<WorkspaceDirectoryEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceSessionRestore {
    pub(crate) workspace: WorkspaceSnapshot,
    pub(crate) active_file: Option<PreparedOpenFileResponse>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceMutation {
    pub(crate) path: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RenameWorkspaceEntryResponse {
    pub(crate) entry_kind: String,
    pub(crate) old_path: String,
    pub(crate) new_path: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeleteWorkspaceEntryResponse {
    pub(crate) deleted_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MutationKind {
    Create,
    Delete,
    Rename,
    Write,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status")]
pub(crate) enum SnapshotReceipt<S> {
    #[serde(rename = "fresh")]
    Fresh { snapshot: S },
    #[serde(rename = "stale")]
    Stale {
        workspace_token: String,
        repair_reason: String,
    },
    #[serde(rename = "not-applicable")]
    NotApplicable,
}

#[derive(Debug, Serialize)]
pub(crate) struct MutationCommitReceipt<T, S> {
    pub(crate) committed: T,
    pub(crate) workspace: SnapshotReceipt<S>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status")]
pub(crate) enum MutationOutcome<T, S> {
    #[serde(rename = "confirmed-not-committed")]
    ConfirmedNotCommitted { message: String },
    #[serde(rename = "confirmed-committed")]
    ConfirmedCommitted {
        receipt: MutationCommitReceipt<T, S>,
    },
    #[serde(rename = "indeterminate")]
    Indeterminate {
        operation: MutationKind,
        paths: Vec<String>,
        recovery_message: String,
    },
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::{json, Value};

    use super::{
        ActiveDocumentDiskSnapshot, ActiveDocumentWatchEvent, ActiveDocumentWatchEventPayload,
        ActiveDocumentWatchHealthStatus, ActiveDocumentWatchReason,
        ActiveDocumentWatchRegistration, ActiveDocumentWatchSnapshotEnvelope,
        MutationCommitReceipt, MutationKind, MutationOutcome, OpenCommitResult, OpenCommitStatus,
        OpenFileResponse, PreparedOpenFileResponse, RecentFileSummary, RecentFilesSnapshot,
        SnapshotReceipt, WorkspaceSessionRestore, WorkspaceSnapshot,
        ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
    };
    use crate::workspace_file_kind::{ContentMode, WorkspaceFileKind};

    #[derive(Serialize)]
    struct FixtureWorkspaceFileEntry {
        kind: WorkspaceFileKind,
        path: &'static str,
        relative_path: &'static str,
        name: &'static str,
    }

    #[derive(Serialize)]
    struct FixtureWorkspaceDirectoryEntry {
        path: &'static str,
        relative_path: &'static str,
        name: &'static str,
    }

    #[derive(Serialize)]
    struct FixtureWorkspaceSnapshot {
        workspace_token: &'static str,
        root: &'static str,
        files: Vec<FixtureWorkspaceFileEntry>,
        directories: Vec<FixtureWorkspaceDirectoryEntry>,
    }

    fn fixture_snapshot() -> FixtureWorkspaceSnapshot {
        FixtureWorkspaceSnapshot {
            workspace_token: "workspace-7",
            root: "/workspace",
            files: vec![
                FixtureWorkspaceFileEntry {
                    kind: WorkspaceFileKind::Markdown,
                    path: "/workspace/notes.md",
                    relative_path: "notes.md",
                    name: "notes.md",
                },
                FixtureWorkspaceFileEntry {
                    kind: WorkspaceFileKind::Html,
                    path: "/workspace/page.xhtml",
                    relative_path: "page.xhtml",
                    name: "page.xhtml",
                },
                FixtureWorkspaceFileEntry {
                    kind: WorkspaceFileKind::Image,
                    path: "/workspace/assets/pixel.png",
                    relative_path: "assets/pixel.png",
                    name: "pixel.png",
                },
                FixtureWorkspaceFileEntry {
                    kind: WorkspaceFileKind::Video,
                    path: "/workspace/assets/clip.mp4",
                    relative_path: "assets/clip.mp4",
                    name: "clip.mp4",
                },
                FixtureWorkspaceFileEntry {
                    kind: WorkspaceFileKind::Audio,
                    path: "/workspace/assets/track.mp3",
                    relative_path: "assets/track.mp3",
                    name: "track.mp3",
                },
            ],
            directories: vec![FixtureWorkspaceDirectoryEntry {
                path: "/workspace/assets",
                relative_path: "assets",
                name: "assets",
            }],
        }
    }

    fn open_file_responses() -> Vec<OpenFileResponse> {
        vec![
            OpenFileResponse {
                kind: WorkspaceFileKind::Markdown,
                path: "/workspace/notes.md".to_string(),
                content_mode: ContentMode::Text,
                content: Some("# Notes".to_string()),
                mime_type: None,
                bytes_base64: None,
            },
            OpenFileResponse {
                kind: WorkspaceFileKind::Html,
                path: "/workspace/page.xhtml".to_string(),
                content_mode: ContentMode::Text,
                content: Some("<main>Page</main>".to_string()),
                mime_type: Some("application/xhtml+xml".to_string()),
                bytes_base64: None,
            },
            OpenFileResponse {
                kind: WorkspaceFileKind::Image,
                path: "/workspace/assets/pixel.png".to_string(),
                content_mode: ContentMode::Binary,
                content: None,
                mime_type: Some("image/png".to_string()),
                bytes_base64: None,
            },
            OpenFileResponse {
                kind: WorkspaceFileKind::Video,
                path: "/workspace/assets/clip.mp4".to_string(),
                content_mode: ContentMode::Binary,
                content: None,
                mime_type: Some("video/mp4".to_string()),
                bytes_base64: None,
            },
            OpenFileResponse {
                kind: WorkspaceFileKind::Audio,
                path: "/workspace/assets/track.mp3".to_string(),
                content_mode: ContentMode::Binary,
                content: None,
                mime_type: Some("audio/mpeg".to_string()),
                bytes_base64: None,
            },
        ]
    }

    #[test]
    fn rust_serde_and_watch_wire_shapes_match_shared_json_fixtures() {
        let canonical: Value = serde_json::from_str(include_str!(
            "../../test-fixtures/tauri-wire/canonical.json"
        ))
        .unwrap();
        let malformed: Value = serde_json::from_str(include_str!(
            "../../test-fixtures/tauri-wire/malformed.json"
        ))
        .unwrap();

        let actual = json!({
            "open_file_responses": open_file_responses(),
            "snapshot_receipts": {
                "fresh": SnapshotReceipt::Fresh {
                    snapshot: fixture_snapshot(),
                },
                "stale": SnapshotReceipt::<FixtureWorkspaceSnapshot>::Stale {
                    workspace_token: "workspace-7".to_string(),
                    repair_reason: "workspace refresh failed".to_string(),
                },
                "not_applicable": SnapshotReceipt::<FixtureWorkspaceSnapshot>::NotApplicable,
            },
            "mutation_outcomes": {
                "confirmed_not_committed":
                    MutationOutcome::<Value, FixtureWorkspaceSnapshot>::ConfirmedNotCommitted {
                        message: "target already exists".to_string(),
                    },
                "confirmed_committed": MutationOutcome::ConfirmedCommitted {
                    receipt: MutationCommitReceipt {
                        committed: json!({ "path": "/workspace/notes.md" }),
                        workspace: SnapshotReceipt::Fresh {
                            snapshot: fixture_snapshot(),
                        },
                    },
                },
                "indeterminate": MutationOutcome::<Value, FixtureWorkspaceSnapshot>::Indeterminate {
                    operation: MutationKind::Rename,
                    paths: vec![
                        "/workspace/old.md".to_string(),
                        "/workspace/new.md".to_string(),
                    ],
                    recovery_message: "reopen the workspace before retrying".to_string(),
                },
            },
            "prepared_open_file_response": PreparedOpenFileResponse {
                file: OpenFileResponse {
                    kind: WorkspaceFileKind::Markdown,
                    path: "/workspace/notes.md".to_string(),
                    content_mode: ContentMode::Text,
                    content: Some("# Notes".to_string()),
                    mime_type: None,
                    bytes_base64: None,
                },
                open_receipt: "11111111111111111111111111111111".to_string(),
                commit_operation_id: "22222222222222222222222222222222".to_string(),
            },
            "recent_files_snapshot": RecentFilesSnapshot {
                entries: vec![
                    RecentFileSummary {
                        id: "33333333333333333333333333333333".to_string(),
                        display_name: "notes.md".to_string(),
                    },
                    RecentFileSummary {
                        id: "44444444444444444444444444444444".to_string(),
                        display_name: "page.xhtml".to_string(),
                    },
                ],
            },
            "open_commit_results": {
                "committed": OpenCommitResult::Committed {
                    recent_files: RecentFilesSnapshot {
                        entries: vec![RecentFileSummary {
                            id: "33333333333333333333333333333333".to_string(),
                            display_name: "notes.md".to_string(),
                        }],
                    },
                },
                "not_committed": OpenCommitResult::NotCommitted {
                    message: "The file could not be finalized. Please try again.".to_string(),
                },
            },
            "open_commit_statuses": {
                "pending": OpenCommitStatus::Pending,
                "committed": OpenCommitStatus::Committed {
                    recent_files: RecentFilesSnapshot {
                        entries: vec![RecentFileSummary {
                            id: "33333333333333333333333333333333".to_string(),
                            display_name: "notes.md".to_string(),
                        }],
                    },
                },
                "not_committed": OpenCommitStatus::NotCommitted {
                    message: "The file could not be finalized. Please try again.".to_string(),
                },
                "unknown": OpenCommitStatus::Unknown,
            },
            "active_document_watch": {
                "registration_present_text": ActiveDocumentWatchRegistration {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-1".to_string(),
                    document_id: "pane-document-1".to_string(),
                    document_generation: 7,
                    sequence: 1,
                    snapshot: ActiveDocumentDiskSnapshot::Present {
                        file: OpenFileResponse {
                            kind: WorkspaceFileKind::Markdown,
                            path: "/workspace/notes.md".to_string(),
                            content_mode: ContentMode::Text,
                            content: Some("# External".to_string()),
                            mime_type: None,
                            bytes_base64: None,
                        },
                        preview_revision: 1,
                    },
                },
                "registration_present_pdf": ActiveDocumentWatchRegistration {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-2".to_string(),
                    document_id: "pane-document-2".to_string(),
                    document_generation: 8,
                    sequence: 1,
                    snapshot: ActiveDocumentDiskSnapshot::Present {
                        file: OpenFileResponse {
                            kind: WorkspaceFileKind::Pdf,
                            path: "/workspace/document.pdf".to_string(),
                            content_mode: ContentMode::Binary,
                            content: None,
                            mime_type: Some("application/pdf".to_string()),
                            bytes_base64: Some("AQ==".to_string()),
                        },
                        preview_revision: 1,
                    },
                },
                "registration_missing": ActiveDocumentWatchRegistration {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-3".to_string(),
                    document_id: "pane-document-3".to_string(),
                    document_generation: 9,
                    sequence: 1,
                    snapshot: ActiveDocumentDiskSnapshot::Missing {
                        path: "/workspace/missing.md".to_string(),
                    },
                },
                "snapshot_resync": ActiveDocumentWatchSnapshotEnvelope {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-1".to_string(),
                    document_id: "pane-document-1".to_string(),
                    document_generation: 7,
                    sequence: 2,
                    reason: ActiveDocumentWatchReason::Resync,
                    previous_path: None,
                    snapshot: ActiveDocumentDiskSnapshot::Present {
                        file: OpenFileResponse {
                            kind: WorkspaceFileKind::Markdown,
                            path: "/workspace/notes.md".to_string(),
                            content_mode: ContentMode::Text,
                            content: Some("# Resynced".to_string()),
                            mime_type: None,
                            bytes_base64: None,
                        },
                        preview_revision: 2,
                    },
                },
                "state_changed": ActiveDocumentWatchEvent {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-1".to_string(),
                    document_id: "pane-document-1".to_string(),
                    document_generation: 7,
                    sequence: 3,
                    event: ActiveDocumentWatchEventPayload::State {
                        reason: ActiveDocumentWatchReason::Changed,
                        previous_path: None,
                        snapshot: ActiveDocumentDiskSnapshot::Present {
                            file: OpenFileResponse {
                                kind: WorkspaceFileKind::Markdown,
                                path: "/workspace/notes.md".to_string(),
                                content_mode: ContentMode::Text,
                                content: Some("# Changed".to_string()),
                                mime_type: None,
                                bytes_base64: None,
                            },
                            preview_revision: 3,
                        },
                    },
                },
                "state_renamed": ActiveDocumentWatchEvent {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-1".to_string(),
                    document_id: "pane-document-1".to_string(),
                    document_generation: 7,
                    sequence: 4,
                    event: ActiveDocumentWatchEventPayload::State {
                        reason: ActiveDocumentWatchReason::Renamed,
                        previous_path: Some("/workspace/notes.md".to_string()),
                        snapshot: ActiveDocumentDiskSnapshot::Present {
                            file: OpenFileResponse {
                                kind: WorkspaceFileKind::Markdown,
                                path: "/workspace/renamed.md".to_string(),
                                content_mode: ContentMode::Text,
                                content: Some("# Renamed".to_string()),
                                mime_type: None,
                                bytes_base64: None,
                            },
                            preview_revision: 4,
                        },
                    },
                },
                "state_missing": ActiveDocumentWatchEvent {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-1".to_string(),
                    document_id: "pane-document-1".to_string(),
                    document_generation: 7,
                    sequence: 5,
                    event: ActiveDocumentWatchEventPayload::State {
                        reason: ActiveDocumentWatchReason::Missing,
                        previous_path: None,
                        snapshot: ActiveDocumentDiskSnapshot::Missing {
                            path: "/workspace/renamed.md".to_string(),
                        },
                    },
                },
                "health_degraded": ActiveDocumentWatchEvent {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-1".to_string(),
                    document_id: "pane-document-1".to_string(),
                    document_generation: 7,
                    sequence: 6,
                    event: ActiveDocumentWatchEventPayload::Health {
                        status: ActiveDocumentWatchHealthStatus::Degraded,
                        message: "Monitoring is temporarily retrying.".to_string(),
                    },
                },
                "health_failed": ActiveDocumentWatchEvent {
                    protocol_version: ACTIVE_DOCUMENT_WATCH_PROTOCOL_VERSION,
                    watch_id: "watch-1".to_string(),
                    document_id: "pane-document-1".to_string(),
                    document_generation: 7,
                    sequence: 7,
                    event: ActiveDocumentWatchEventPayload::Health {
                        status: ActiveDocumentWatchHealthStatus::Failed,
                        message: "Monitoring stopped. Reopen the file to retry.".to_string(),
                    },
                },
            },
        });

        assert_eq!(actual, canonical);
        assert_ne!(
            actual["snapshot_receipts"]["fresh"],
            malformed["externally_tagged_snapshot_receipt"]
        );
        assert_ne!(
            actual["mutation_outcomes"]["confirmed_committed"],
            malformed["tuple_mutation_outcome"]
        );
    }

    #[test]
    fn workspace_session_restore_always_serializes_an_active_file_field() {
        let restore = WorkspaceSessionRestore {
            workspace: WorkspaceSnapshot {
                workspace_token: "workspace-7".to_string(),
                root: "/workspace".to_string(),
                files: Vec::new(),
                directories: Vec::new(),
            },
            active_file: None,
        };

        assert_eq!(
            serde_json::to_value(restore).unwrap(),
            json!({
                "workspace": {
                    "workspace_token": "workspace-7",
                    "root": "/workspace",
                    "files": [],
                    "directories": [],
                },
                "active_file": null,
            })
        );
    }
}
