export type WorkspaceFileKind =
  | 'markdown'
  | 'html'
  | 'excalidraw'
  | 'image'
  | 'video'
  | 'audio'
  | 'pdf'
  | 'docx';
export type ContentMode = 'text' | 'binary';

export interface WorkspaceFileEntry {
  kind: WorkspaceFileKind;
  path: string;
  relative_path: string;
  name: string;
}

export interface WorkspaceDirectoryEntry {
  path: string;
  relative_path: string;
  name: string;
}

export interface OpenMarkdownFileResponse {
  kind: 'markdown';
  path: string;
  content_mode: 'text';
  content: string;
  mime_type?: never;
}

export interface OpenImageFileResponse {
  kind: 'image';
  path: string;
  content_mode: 'binary';
  content?: never;
  mime_type: string;
}

export interface OpenHtmlFileResponse {
  kind: 'html';
  path: string;
  content_mode: 'text';
  content: string;
  mime_type: string;
}

export interface OpenExcalidrawFileResponse {
  kind: 'excalidraw';
  path: string;
  content_mode: 'text';
  content: string;
  mime_type?: never;
}

export interface OpenMediaFileResponse {
  kind: 'video' | 'audio';
  path: string;
  content_mode: 'binary';
  content?: never;
  mime_type: string;
}

export interface OpenBinaryDocumentResponse {
  kind: 'pdf' | 'docx';
  path: string;
  content_mode: 'binary';
  content?: never;
  mime_type: string;
  bytes_base64: string;
}

export type OpenFileResponse =
  | OpenMarkdownFileResponse
  | OpenHtmlFileResponse
  | OpenExcalidrawFileResponse
  | OpenImageFileResponse
  | OpenMediaFileResponse
  | OpenBinaryDocumentResponse;

export interface PreparedOpenFileResponse {
  file: OpenFileResponse;
  open_receipt: string;
  commit_operation_id: string;
}

export interface WorkspaceSessionRestore {
  workspace: WorkspaceSnapshot;
  active_file: PreparedOpenFileResponse | null;
}

export interface RecentFileSummary {
  id: string;
  display_name: string;
}

export interface RecentFilesSnapshot {
  entries: RecentFileSummary[];
}

export type OpenCommitResult =
  | { status: 'committed'; recent_files: RecentFilesSnapshot }
  | { status: 'not_committed'; message: string };

export type OpenCommitStatus =
  | { status: 'pending' }
  | { status: 'committed'; recent_files: RecentFilesSnapshot }
  | { status: 'not_committed'; message: string }
  | { status: 'unknown' };

export interface WorkspaceDirectoryListing {
  root: string;
  files: WorkspaceFileEntry[];
  directories: WorkspaceDirectoryEntry[];
}

export interface WorkspaceSnapshot extends WorkspaceDirectoryListing {
  workspace_token: string;
}

export interface WorkspaceMutation {
  path: string;
}

export type SnapshotReceipt =
  | { status: 'fresh'; snapshot: WorkspaceSnapshot }
  | { status: 'stale'; workspace_token: string; repair_reason: string }
  | { status: 'not-applicable' };

export interface MutationCommitReceipt<T> {
  committed: T;
  workspace: SnapshotReceipt;
}

export type MutationKind = 'create' | 'delete' | 'rename' | 'write';

export type MutationOutcome<T> =
  | { status: 'confirmed-not-committed'; message: string }
  | { status: 'confirmed-committed'; receipt: MutationCommitReceipt<T> }
  | { status: 'indeterminate'; operation: MutationKind; paths: string[]; recovery_message: string };

export interface RenameWorkspaceEntryResponse {
  entry_kind: 'file' | 'directory';
  new_path: string;
  old_path: string;
}

export interface DeleteWorkspaceEntryResponse {
  deleted_path: string;
}
