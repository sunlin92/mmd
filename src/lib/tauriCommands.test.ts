import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  createWorkspaceDirectory,
  createWorkspaceFile,
  deleteWorkspaceEntry,
  moveWorkspaceEntry,
  openDirectoryDialog,
  openFileDialog,
  persistWorkspaceSession,
  prepareHtmlPreview,
  readWorkspaceImage,
  refreshDirectory,
  renameWorkspaceEntry,
  resolveWorkspaceMedia,
  saveAsDialog,
  setNativeSaveMenuEnabled,
  setNativeLocalePreference,
  setNativeThemePreference,
  restoreWorkspaceSession,
  writeFile,
} from './tauriCommands';

const invokeMock = vi.hoisted(() => vi.fn<(command: string, payload?: unknown) => Promise<unknown>>());

const workspaceSnapshot = {
  workspace_token: 'workspace-7',
  root: '/workspace',
  files: [],
  directories: [],
};

const committedCreateOutcome = {
  status: 'confirmed-committed',
  receipt: {
    committed: {
      kind: 'markdown',
      path: '/workspace/draft.md',
      content_mode: 'text',
      content: '',
    },
    workspace: {
      status: 'stale',
      workspace_token: 'workspace-7',
      repair_reason: 'injected post-commit snapshot failure',
    },
  },
};

const committedDirectoryOutcome = {
  status: 'confirmed-committed',
  receipt: {
    committed: {
      path: '/workspace/drafts',
    },
    workspace: {
      status: 'stale',
      workspace_token: 'workspace-7',
      repair_reason: 'injected post-commit snapshot failure',
    },
  },
};

const committedRenameOutcome = {
  status: 'confirmed-committed',
  receipt: {
    committed: {
      entry_kind: 'file',
      old_path: '/workspace/draft.md',
      new_path: '/workspace/renamed.md',
    },
    workspace: {
      status: 'stale',
      workspace_token: 'workspace-7',
      repair_reason: 'injected post-commit snapshot failure',
    },
  },
};

const committedDeleteOutcome = {
  status: 'confirmed-committed',
  receipt: {
    committed: {
      deleted_path: '/workspace/drafts',
    },
    workspace: {
      status: 'stale',
      workspace_token: 'workspace-7',
      repair_reason: 'injected post-commit snapshot failure',
    },
  },
};

const committedMoveOutcome = {
  status: 'confirmed-committed',
  receipt: {
    committed: {
      entry_kind: 'file',
      old_path: '/workspace/draft.md',
      new_path: '/workspace/archive/draft.md',
    },
    workspace: {
      status: 'stale',
      workspace_token: 'workspace-7',
      repair_reason: 'injected post-commit snapshot failure',
    },
  },
};

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

describe('Tauri command wrappers', () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it('syncs the validated frontend theme preference into the native menu projection', async () => {
    invokeMock.mockResolvedValue(undefined);

    await setNativeThemePreference({
      version: 1,
      selectedSkin: 'songke-zhuying',
      followSystem: true,
    });

    expect(invokeMock).toHaveBeenCalledWith('set_native_theme_preference', {
      selectedSkin: 'songke-zhuying',
      followSystem: true,
    });
  });

  it('syncs locale mode and effective language into the native menu projection', async () => {
    invokeMock.mockResolvedValue(undefined);

    await setNativeLocalePreference({ version: 1, mode: 'system' }, 'zh-CN');

    expect(invokeMock).toHaveBeenCalledWith('set_native_locale_preference', {
      mode: 'system',
      effectiveLocale: 'zh-CN',
    });
  });

  it('rejects a legacy null write response before treating the save as complete', async () => {
    invokeMock.mockResolvedValue(null);

    await expect(writeFile('/workspace/draft.md', '# Saved')).rejects.toThrow('Invalid mutation outcome');
    expect(invokeMock).toHaveBeenCalledWith('write_file', {
      path: '/workspace/draft.md',
      content: '# Saved',
    });
  });

  it('rejects a confirmed-not-committed write with the backend message', async () => {
    invokeMock.mockResolvedValue({
      status: 'confirmed-not-committed',
      message: 'injected pre-call write failure',
    });

    await expect(writeFile('/workspace/draft.md', '# Saved')).rejects.toThrow(
      'injected pre-call write failure',
    );
  });

  it('rejects an indeterminate write with the recovery message', async () => {
    invokeMock.mockResolvedValue({
      status: 'indeterminate',
      operation: 'write',
      paths: ['/workspace/draft.md'],
      recovery_message: 'Reopen and inspect the file before retrying.',
    });

    await expect(writeFile('/workspace/draft.md', '# Saved')).rejects.toThrow(
      'Reopen and inspect the file before retrying.',
    );
  });

  it('resolves a canonical committed write outcome to void', async () => {
    invokeMock.mockResolvedValue({
      status: 'confirmed-committed',
      receipt: {
        committed: { path: '/workspace/draft.md' },
        workspace: { status: 'not-applicable' },
      },
    });

    await expect(writeFile('/workspace/draft.md', '# Saved')).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenCalledWith('write_file', {
      path: '/workspace/draft.md',
      content: '# Saved',
    });
  });

  it('calls exact workspace mutation command names with stable payloads', async () => {
    invokeMock
      .mockResolvedValueOnce(committedCreateOutcome)
      .mockResolvedValueOnce(workspaceSnapshot)
      .mockResolvedValueOnce(committedDirectoryOutcome)
      .mockResolvedValueOnce(committedRenameOutcome)
      .mockResolvedValueOnce(committedMoveOutcome)
      .mockResolvedValue(committedDeleteOutcome);

    await expect(createWorkspaceFile('workspace-7', '/workspace', 'draft.md')).resolves.toEqual(
      committedCreateOutcome,
    );
    await expect(refreshDirectory('workspace-7', '/workspace')).resolves.toEqual(workspaceSnapshot);
    await expect(createWorkspaceDirectory('workspace-7', '/workspace', 'drafts')).resolves.toEqual(
      committedDirectoryOutcome,
    );
    await expect(renameWorkspaceEntry('workspace-7', '/workspace/draft.md', 'renamed.md')).resolves.toEqual(
      committedRenameOutcome,
    );
    await expect(moveWorkspaceEntry('workspace-7', '/workspace/draft.md', '/workspace/archive')).resolves.toEqual(
      committedMoveOutcome,
    );
    await expect(deleteWorkspaceEntry('workspace-7', '/workspace/drafts')).resolves.toEqual(
      committedDeleteOutcome,
    );

    expect(invokeMock).toHaveBeenNthCalledWith(1, 'create_workspace_file', {
      workspaceToken: 'workspace-7',
      parentPath: '/workspace',
      name: 'draft.md',
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'refresh_directory', {
      workspaceToken: 'workspace-7',
      path: '/workspace',
    });
    expect(invokeMock).toHaveBeenNthCalledWith(3, 'create_workspace_directory', {
      workspaceToken: 'workspace-7',
      parentPath: '/workspace',
      name: 'drafts',
    });
    expect(invokeMock).toHaveBeenNthCalledWith(4, 'rename_workspace_entry', {
      workspaceToken: 'workspace-7',
      path: '/workspace/draft.md',
      newName: 'renamed.md',
    });
    expect(invokeMock).toHaveBeenNthCalledWith(5, 'move_workspace_entry', {
      workspaceToken: 'workspace-7',
      path: '/workspace/draft.md',
      destinationParentPath: '/workspace/archive',
    });
    expect(invokeMock).toHaveBeenNthCalledWith(6, 'delete_workspace_entry', {
      workspaceToken: 'workspace-7',
      path: '/workspace/drafts',
    });

    invokeMock.mockResolvedValueOnce({ status: 'confirmed-committed' });
    await expect(createWorkspaceFile('workspace-7', '/workspace', 'invalid.md')).rejects.toThrow(
      'Invalid mutation outcome',
    );

    invokeMock.mockResolvedValueOnce({
      status: 'confirmed-committed',
      receipt: {
        committed: { path: 7 },
        workspace: { status: 'not-applicable' },
      },
    });
    await expect(createWorkspaceDirectory('workspace-7', '/workspace', 'invalid')).rejects.toThrow(
      'Invalid mutation outcome',
    );

    invokeMock.mockResolvedValueOnce({
      ...committedRenameOutcome,
      receipt: {
        ...committedRenameOutcome.receipt,
        committed: {
          ...committedRenameOutcome.receipt.committed,
          directory: workspaceSnapshot,
        },
      },
    });
    await expect(renameWorkspaceEntry('workspace-7', '/workspace/draft.md', 'renamed.md')).rejects.toThrow(
      'Invalid mutation outcome',
    );

    invokeMock.mockResolvedValueOnce({
      ...committedDeleteOutcome,
      receipt: {
        ...committedDeleteOutcome.receipt,
        committed: { deletedPath: '/workspace/drafts' },
      },
    });
    await expect(deleteWorkspaceEntry('workspace-7', '/workspace/drafts')).rejects.toThrow(
      'Invalid mutation outcome',
    );
  });

  it('decodes an indeterminate create-file outcome without claiming a commit', async () => {
    const outcome = {
      status: 'indeterminate',
      operation: 'create',
      paths: ['/workspace/draft.md'],
      recovery_message: 'Refresh and inspect the workspace before retrying.',
    };
    invokeMock.mockResolvedValue(outcome);

    await expect(createWorkspaceFile('workspace-7', '/workspace', 'draft.md')).resolves.toEqual(outcome);
    expect(invokeMock).toHaveBeenCalledWith('create_workspace_file', {
      workspaceToken: 'workspace-7',
      parentPath: '/workspace',
      name: 'draft.md',
    });
  });

  it('passes the explicit Excalidraw kind when creating a standard drawing', async () => {
    const outcome = {
      status: 'confirmed-committed',
      receipt: {
        committed: {
          kind: 'excalidraw',
          path: '/workspace/architecture.excalidraw',
          content_mode: 'text',
          content: '{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}',
        },
        workspace: { status: 'not-applicable' },
      },
    } as const;
    invokeMock.mockResolvedValue(outcome);

    await expect(createWorkspaceFile(
      'workspace-7',
      '/workspace',
      'architecture',
      'excalidraw',
    )).resolves.toEqual(outcome);
    expect(invokeMock).toHaveBeenCalledWith('create_workspace_file', {
      fileKind: 'excalidraw',
      workspaceToken: 'workspace-7',
      parentPath: '/workspace',
      name: 'architecture',
    });
  });

  it('calls stable dialog command names with their existing request fields', async () => {
    invokeMock
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(workspaceSnapshot)
      .mockResolvedValueOnce(null);

    await openFileDialog();
    await expect(openDirectoryDialog()).resolves.toEqual(workspaceSnapshot);
    await saveAsDialog('# Draft', 'Draft.md');

    expect(invokeMock).toHaveBeenNthCalledWith(1, 'open_file_dialog');
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'open_directory_dialog');
    expect(invokeMock).toHaveBeenNthCalledWith(3, 'save_as_dialog', {
      content: '# Draft',
      defaultName: 'Draft.md',
    });
  });

  it('strictly decodes and persists workspace session state through stable commands', async () => {
    const activeFile = {
      file: {
        kind: 'markdown',
        path: '/workspace/notes.md',
        content_mode: 'text',
        content: '# Notes',
      },
      open_receipt: '0123456789abcdef0123456789abcdef',
      commit_operation_id: 'fedcba9876543210fedcba9876543210',
    };
    invokeMock
      .mockResolvedValueOnce({ workspace: workspaceSnapshot, active_file: activeFile })
      .mockResolvedValueOnce(undefined);

    await expect(restoreWorkspaceSession()).resolves.toEqual({
      workspace: workspaceSnapshot,
      active_file: activeFile,
    });
    await expect(persistWorkspaceSession(
      'workspace-7',
      '/workspace',
      '/workspace/notes.md',
    )).resolves.toBeUndefined();

    expect(invokeMock).toHaveBeenNthCalledWith(1, 'restore_workspace_session');
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'persist_workspace_session', {
      workspaceToken: 'workspace-7',
      workspaceRoot: '/workspace',
      activePath: '/workspace/notes.md',
    });

    invokeMock.mockResolvedValueOnce({ workspace: workspaceSnapshot });
    await expect(restoreWorkspaceSession()).rejects.toThrow('Invalid workspace session restore');
  });

  it('updates native Save and Save As availability through one focused command', async () => {
    invokeMock.mockResolvedValue(undefined);

    await expect(setNativeSaveMenuEnabled(false)).resolves.toBeUndefined();

    expect(invokeMock).toHaveBeenCalledWith('set_native_save_menu_enabled', {
      enabled: false,
    });
  });

  it('decodes save-as mutation outcomes instead of collapsing them to a path', async () => {
    const outcome = {
      status: 'confirmed-committed',
      receipt: {
        committed: { path: '/workspace/saved.md' },
        workspace: { status: 'not-applicable' },
      },
    };
    invokeMock.mockResolvedValue(outcome);

    await expect(saveAsDialog('# Draft', 'Draft.md')).resolves.toEqual(outcome);
    expect(invokeMock).toHaveBeenCalledWith('save_as_dialog', {
      content: '# Draft',
      defaultName: 'Draft.md',
    });
  });

  it('restricts an Excalidraw Save As request to the Excalidraw backend flow', async () => {
    const outcome = {
      status: 'confirmed-committed',
      receipt: {
        committed: { path: '/workspace/architecture.excalidraw' },
        workspace: { status: 'not-applicable' },
      },
    };
    invokeMock.mockResolvedValue(outcome);

    await expect(saveAsDialog(
      '{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}',
      'architecture.excalidraw',
      'excalidraw',
    )).resolves.toEqual(outcome);
    expect(invokeMock).toHaveBeenCalledWith('save_as_dialog', {
      content: '{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}',
      defaultName: 'architecture.excalidraw',
      fileKind: 'excalidraw',
    });
  });

  it('preserves save-as cancellation as null', async () => {
    invokeMock.mockResolvedValue(null);

    await expect(saveAsDialog('# Draft', 'Draft.md')).resolves.toBeNull();
  });

  it('reads an authorized workspace image as a browser-safe data URL', async () => {
    invokeMock.mockResolvedValue('data:image/png;base64,iVBORw==');

    await expect(readWorkspaceImage('/workspace/assets/cover.png')).resolves.toBe('data:image/png;base64,iVBORw==');
    expect(invokeMock).toHaveBeenCalledWith('read_workspace_image', {
      path: '/workspace/assets/cover.png',
    });
  });

  it('resolves authorized media before creating an asset URL', async () => {
    invokeMock.mockResolvedValue('/workspace/media/clip.mp4');

    await expect(resolveWorkspaceMedia('/workspace/media/clip.mp4')).resolves.toBe('/workspace/media/clip.mp4');
    expect(invokeMock).toHaveBeenCalledWith('resolve_workspace_media', {
      path: '/workspace/media/clip.mp4',
    });
  });

  it('prepares live HTML content on the loopback preview server', async () => {
    invokeMock.mockResolvedValue('http://127.0.0.1:43127/site/index.html');

    await expect(prepareHtmlPreview('/workspace/site/index.html', '<h1>Draft</h1>')).resolves.toBe('http://127.0.0.1:43127/site/index.html');
    expect(invokeMock).toHaveBeenCalledWith('prepare_html_preview', {
      path: '/workspace/site/index.html',
      content: '<h1>Draft</h1>',
    });
  });

});
