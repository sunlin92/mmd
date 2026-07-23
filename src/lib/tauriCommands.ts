import { invoke } from '@tauri-apps/api/core';
import type {
  DeleteWorkspaceEntryResponse,
  MutationOutcome,
  OpenCommitResult,
  OpenCommitStatus,
  OpenFileResponse,
  PreparedOpenFileResponse,
  RecentFilesSnapshot,
  RenameWorkspaceEntryResponse,
  WorkspaceSessionRestore,
  WorkspaceFileKind,
  WorkspaceMutation,
  WorkspaceSnapshot,
} from '../types';
import {
  decodeDeleteWorkspaceEntryResponse,
  decodeMutationOutcome,
  decodeOpenFileResponse,
  decodeRenameWorkspaceEntryResponse,
  decodeWorkspaceMutation,
  decodeWorkspaceSnapshot,
} from './workspaceFileKind';
import {
  decodeOpenCommitResult,
  decodeOpenCommitStatus,
  decodePreparedOpenFileResponse,
  decodeRecentFilesSnapshot,
} from './recentFiles';
import { decodeWorkspaceSessionRestore } from './workspaceSession';
import type { ThemePreference } from './theme';
import type { EffectiveLocale, LocalePreference } from './locale';

export async function refreshDirectory(workspaceToken: string, path: string): Promise<WorkspaceSnapshot> {
  const response = await invoke<unknown>('refresh_directory', { workspaceToken, path });
  return decodeWorkspaceSnapshot(response);
}

export async function openWorkspaceFile(path: string): Promise<PreparedOpenFileResponse> {
  const response = await invoke<unknown>('open_workspace_file', { path });
  return decodePreparedOpenFileResponse(response);
}

export async function openFileDialog(): Promise<PreparedOpenFileResponse | null> {
  const response = await invoke<unknown>('open_file_dialog');
  return response === null ? null : decodePreparedOpenFileResponse(response);
}

export async function listRecentFiles(): Promise<RecentFilesSnapshot> {
  return decodeRecentFilesSnapshot(await invoke<unknown>('list_recent_files'));
}

export async function openRecentFile(entryId: string): Promise<PreparedOpenFileResponse> {
  const response = await invoke<unknown>('open_recent_file', { entryId });
  return decodePreparedOpenFileResponse(response);
}

export async function commitRecentOpen(openReceipt: string): Promise<OpenCommitResult> {
  const response = await invoke<unknown>('commit_recent_open', { openReceipt });
  return decodeOpenCommitResult(response);
}

export async function getOpenCommitStatus(commitOperationId: string): Promise<OpenCommitStatus> {
  const response = await invoke<unknown>('get_open_commit_status', { commitOperationId });
  return decodeOpenCommitStatus(response);
}

export function discardOpenReceipt(openReceipt: string): Promise<boolean> {
  return invoke<boolean>('discard_open_receipt', { openReceipt });
}

export async function removeRecentFile(entryId: string): Promise<RecentFilesSnapshot> {
  const response = await invoke<unknown>('remove_recent_file', { entryId });
  return decodeRecentFilesSnapshot(response);
}

export async function clearRecentFiles(): Promise<RecentFilesSnapshot> {
  const response = await invoke<unknown>('clear_recent_files');
  return decodeRecentFilesSnapshot(response);
}

export function setNativeSaveMenuEnabled(enabled: boolean): Promise<void> {
  return invoke<void>('set_native_save_menu_enabled', { enabled });
}

export function setNativeThemePreference(preference: ThemePreference): Promise<void> {
  return invoke<void>('set_native_theme_preference', {
    selectedSkin: preference.selectedSkin,
    followSystem: preference.followSystem,
  });
}

export function setNativeLocalePreference(
  preference: LocalePreference,
  effectiveLocale: EffectiveLocale,
): Promise<void> {
  return invoke<void>('set_native_locale_preference', {
    mode: preference.mode,
    effectiveLocale,
  });
}

export async function openDirectoryDialog(): Promise<WorkspaceSnapshot | null> {
  const response = await invoke<unknown>('open_directory_dialog');
  return response === null ? null : decodeWorkspaceSnapshot(response);
}

export async function restoreWorkspaceSession(): Promise<WorkspaceSessionRestore | null> {
  const response = await invoke<unknown>('restore_workspace_session');
  return decodeWorkspaceSessionRestore(response);
}

export function persistWorkspaceSession(
  workspaceToken: string,
  workspaceRoot: string,
  activePath: string | null,
): Promise<void> {
  return invoke<void>('persist_workspace_session', { workspaceToken, workspaceRoot, activePath });
}

export async function saveAsDialog(
  content: string,
  defaultName: string,
  fileKind?: Extract<WorkspaceFileKind, 'excalidraw'>,
): Promise<MutationOutcome<WorkspaceMutation> | null> {
  const response = await invoke<unknown>('save_as_dialog', {
    content,
    defaultName,
    ...(fileKind ? { fileKind } : {}),
  });
  return response === null ? null : decodeMutationOutcome(response, decodeWorkspaceMutation);
}

export async function writeFile(path: string, content: string): Promise<void> {
  const response = await invoke<unknown>('write_file', { path, content });
  const outcome = decodeMutationOutcome(response, decodeWorkspaceMutation);
  if (outcome.status === 'confirmed-not-committed') {
    throw new Error(outcome.message);
  }
  if (outcome.status === 'indeterminate') {
    throw new Error(outcome.recovery_message);
  }
}

export async function createWorkspaceFile(
  workspaceToken: string,
  parentPath: string,
  name: string,
  fileKind: Extract<WorkspaceFileKind, 'markdown' | 'excalidraw'> = 'markdown',
): Promise<MutationOutcome<OpenFileResponse>> {
  const response = await invoke<unknown>('create_workspace_file', {
    workspaceToken,
    parentPath,
    name,
    ...(fileKind === 'excalidraw' ? { fileKind } : {}),
  });
  return decodeMutationOutcome(response, decodeOpenFileResponse);
}

export async function createWorkspaceDirectory(
  workspaceToken: string,
  parentPath: string,
  name: string,
): Promise<MutationOutcome<WorkspaceMutation>> {
  const response = await invoke<unknown>('create_workspace_directory', { workspaceToken, parentPath, name });
  return decodeMutationOutcome(response, decodeWorkspaceMutation);
}

export async function renameWorkspaceEntry(
  workspaceToken: string,
  path: string,
  newName: string,
): Promise<MutationOutcome<RenameWorkspaceEntryResponse>> {
  const response = await invoke<unknown>('rename_workspace_entry', { workspaceToken, path, newName });
  return decodeMutationOutcome(response, decodeRenameWorkspaceEntryResponse);
}

export async function moveWorkspaceEntry(
  workspaceToken: string,
  path: string,
  destinationParentPath: string,
): Promise<MutationOutcome<RenameWorkspaceEntryResponse>> {
  const response = await invoke<unknown>('move_workspace_entry', {
    workspaceToken,
    path,
    destinationParentPath,
  });
  return decodeMutationOutcome(response, decodeRenameWorkspaceEntryResponse);
}

export async function deleteWorkspaceEntry(
  workspaceToken: string,
  path: string,
): Promise<MutationOutcome<DeleteWorkspaceEntryResponse>> {
  const response = await invoke<unknown>('delete_workspace_entry', { workspaceToken, path });
  return decodeMutationOutcome(response, decodeDeleteWorkspaceEntryResponse);
}

export function readWorkspaceImage(path: string): Promise<string> {
  return invoke<string>('read_workspace_image', { path });
}

export function resolveWorkspaceMedia(path: string): Promise<string> {
  return invoke<string>('resolve_workspace_media', { path });
}

export function prepareHtmlPreview(path: string, content: string): Promise<string> {
  return invoke<string>('prepare_html_preview', { path, content });
}

export interface MarkdownHtmlEmbedLease {
  url: string;
  ownerId: number;
}

export function prepareMarkdownHtmlEmbed(
  markdownPath: string,
  htmlSrc: string,
  workspaceRoot: string | null,
): Promise<MarkdownHtmlEmbedLease> {
  return invoke<MarkdownHtmlEmbedLease>('prepare_markdown_html_embed', {
    markdownPath,
    htmlSrc,
    workspaceRoot,
  });
}

export function releaseMarkdownHtmlEmbed(ownerId: number): Promise<void> {
  return invoke<void>('release_markdown_html_embed', { ownerId });
}
