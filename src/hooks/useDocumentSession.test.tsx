// @vitest-environment jsdom

import { act, StrictMode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { EMPTY_MARKDOWN } from '../lib/documentNames';
import type {
  DocumentSessionState,
} from '../lib/documentSession';
import type { PaneSnapshotEnvelope } from '../lib/paneSync';
import type {
  OpenCommitResult,
  PreparedOpenFileResponse,
  WorkspaceSnapshot,
} from '../types';
import { useDocumentSession } from './useDocumentSession';

interface FakePaneReplicationOptions {
  observe: (snapshot: PaneSnapshotEnvelope) => void;
  role: 'main' | 'editor-popout' | 'preview-popout';
}

interface FakePaneReplication {
  options: FakePaneReplicationOptions;
  dispose: () => void;
  publishAuthoritativeState: (state: unknown) => void;
  publishEditorContent: (content: string) => void;
  start: () => void;
}

const restoreDocumentSnapshotMock = vi.hoisted(() => (
  vi.fn<(snapshot: DocumentSessionState) => DocumentSessionState>()
));

const tauriMocks = vi.hoisted(() => ({
  clearRecentFiles: vi.fn<typeof import('../lib/tauriCommands').clearRecentFiles>(),
  commitRecentOpen: vi.fn<typeof import('../lib/tauriCommands').commitRecentOpen>(),
  createWorkspaceDirectory: vi.fn<typeof import('../lib/tauriCommands').createWorkspaceDirectory>(),
  createWorkspaceFile: vi.fn<typeof import('../lib/tauriCommands').createWorkspaceFile>(),
  deleteWorkspaceEntry: vi.fn<typeof import('../lib/tauriCommands').deleteWorkspaceEntry>(),
  discardOpenReceipt: vi.fn<typeof import('../lib/tauriCommands').discardOpenReceipt>(),
  getOpenCommitStatus: vi.fn<typeof import('../lib/tauriCommands').getOpenCommitStatus>(),
  moveWorkspaceEntry: vi.fn<typeof import('../lib/tauriCommands').moveWorkspaceEntry>(),
  openDirectoryDialog: vi.fn<typeof import('../lib/tauriCommands').openDirectoryDialog>(),
  openFileDialog: vi.fn<typeof import('../lib/tauriCommands').openFileDialog>(),
  openRecentFile: vi.fn<typeof import('../lib/tauriCommands').openRecentFile>(),
  openWorkspaceFile: vi.fn<typeof import('../lib/tauriCommands').openWorkspaceFile>(),
  persistWorkspaceSession: vi.fn<typeof import('../lib/tauriCommands').persistWorkspaceSession>(),
  refreshDirectory: vi.fn<typeof import('../lib/tauriCommands').refreshDirectory>(),
  renameWorkspaceEntry: vi.fn<typeof import('../lib/tauriCommands').renameWorkspaceEntry>(),
  restoreWorkspaceSession: vi.fn<typeof import('../lib/tauriCommands').restoreWorkspaceSession>(),
  saveAsDialog: vi.fn<typeof import('../lib/tauriCommands').saveAsDialog>(),
  writeFile: vi.fn<typeof import('../lib/tauriCommands').writeFile>(),
}));

const paneMocks = vi.hoisted(() => ({
  createPaneProtocolId: vi.fn<(prefix: string) => string>(),
  createTauriPaneReplication: vi.fn<(
    options: FakePaneReplicationOptions,
  ) => FakePaneReplication>(),
  instances: [] as FakePaneReplication[],
}));

vi.mock('../lib/documentSession', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../lib/documentSession')>();
  return {
    ...actual,
    restoreDocumentSnapshot: restoreDocumentSnapshotMock,
  };
});

vi.mock('../lib/tauriCommands', () => tauriMocks);

vi.mock('../lib/tauriPaneReplication', () => ({
  createPaneProtocolId: paneMocks.createPaneProtocolId,
  createTauriPaneReplication: paneMocks.createTauriPaneReplication,
}));

type Session = ReturnType<typeof useDocumentSession>;
type OpenKind = 'normal' | 'recent';

interface Deferred<T> {
  promise: Promise<T>;
  reject: (reason?: unknown) => void;
  resolve: (value: T) => void;
}

let currentSession: Session | null = null;

function SessionHarness({ isPopout = false, popoutPane = 'main' }: {
  isPopout?: boolean;
  popoutPane?: 'main' | 'editor' | 'preview';
}) {
  currentSession = useDocumentSession({
    activeDocumentWatchTransport: null,
    isPopout,
    popoutPane,
  });
  return null;
}

function session(): Session {
  if (!currentSession) throw new Error('Expected a mounted document session');
  return currentSession;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function preparedOpen(
  name: string,
  file: PreparedOpenFileResponse['file'] = {
    kind: 'markdown',
    path: `/workspace/${name}.md`,
    content_mode: 'text',
    content: `# ${name}`,
  },
): PreparedOpenFileResponse {
  return {
    file,
    open_receipt: `${name}-open-receipt`,
    commit_operation_id: `${name}-commit-operation`,
  };
}

function committed(): OpenCommitResult {
  return { status: 'committed', recent_files: { entries: [] } };
}

function workspaceSnapshot(
  files: WorkspaceSnapshot['files'] = [],
): WorkspaceSnapshot {
  return {
    workspace_token: 'restored-workspace-token',
    root: '/workspace',
    files,
    directories: [],
  };
}

function enableTauriRuntime() {
  Object.defineProperty(window, '__TAURI_INTERNALS__', {
    configurable: true,
    value: {},
  });
}

function disableTauriRuntime() {
  delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
}

async function flushSessionEffects() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function documentSnapshot(value: Session) {
  return {
    activeFileKind: value.activeFileKind,
    activeMimeType: value.activeMimeType,
    activePath: value.activePath,
    authorityStatus: value.authorityStatus,
    content: value.content,
    dirty: value.dirty,
    previewRevision: 0,
    documentEpoch: value.documentEpoch,
    documentId: value.documentId,
    lastSavedContent: value.lastSavedContent,
  };
}

async function beginOpen(
  kind: OpenKind,
  entryId = 'recent-entry',
): Promise<{ completion: Promise<void> }> {
  let openPromise!: Promise<void>;
  await act(async () => {
    openPromise = kind === 'normal'
      ? session().handleOpenFile()
      : session().handleOpenRecent(entryId);
    await Promise.resolve();
    await Promise.resolve();
  });
  return { completion: openPromise };
}

function provisionalPaneSnapshot(): PaneSnapshotEnvelope {
  return {
    protocolVersion: 2,
    authorityId: 'main-authority',
    revision: 1,
    documentId: 'provisional-document',
    documentEpoch: 9,
    state: {
      activeFileKind: 'markdown',
      activeMimeType: null,
      activePath: '/workspace/provisional.md',
      authorityStatus: 'provisional',
      content: '# Provisional',
      lastSavedContent: '# Provisional',
      workspaceRoot: '/workspace',
      documentId: 'provisional-document',
      previewRevision: 0,
      documentEpoch: 9,
    },
  };
}

describe('useDocumentSession prepared-open authority workflow', () => {
  let container: HTMLDivElement;
  let root: Root;
  let nextPaneId: number;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    currentSession = null;
    nextPaneId = 0;

    restoreDocumentSnapshotMock.mockReset();
    restoreDocumentSnapshotMock.mockImplementation((snapshot: DocumentSessionState) => ({ ...snapshot }));

    tauriMocks.clearRecentFiles.mockReset();
    tauriMocks.commitRecentOpen.mockReset();
    tauriMocks.createWorkspaceDirectory.mockReset();
    tauriMocks.createWorkspaceFile.mockReset();
    tauriMocks.deleteWorkspaceEntry.mockReset();
    tauriMocks.discardOpenReceipt.mockReset();
    tauriMocks.getOpenCommitStatus.mockReset();
    tauriMocks.moveWorkspaceEntry.mockReset();
    tauriMocks.openDirectoryDialog.mockReset();
    tauriMocks.openFileDialog.mockReset();
    tauriMocks.openRecentFile.mockReset();
    tauriMocks.openWorkspaceFile.mockReset();
    tauriMocks.persistWorkspaceSession.mockReset();
    tauriMocks.refreshDirectory.mockReset();
    tauriMocks.renameWorkspaceEntry.mockReset();
    tauriMocks.restoreWorkspaceSession.mockReset();
    tauriMocks.saveAsDialog.mockReset();
    tauriMocks.writeFile.mockReset();

    tauriMocks.clearRecentFiles.mockResolvedValue({ entries: [] });
    tauriMocks.commitRecentOpen.mockResolvedValue(committed());
    tauriMocks.discardOpenReceipt.mockResolvedValue(true);
    tauriMocks.getOpenCommitStatus.mockResolvedValue({ status: 'unknown' });
    tauriMocks.openFileDialog.mockResolvedValue(null);
    tauriMocks.persistWorkspaceSession.mockResolvedValue(undefined);
    tauriMocks.restoreWorkspaceSession.mockResolvedValue(null);
    tauriMocks.saveAsDialog.mockResolvedValue(null);
    tauriMocks.writeFile.mockResolvedValue(undefined);

    paneMocks.instances.splice(0);
    paneMocks.createPaneProtocolId.mockReset();
    paneMocks.createTauriPaneReplication.mockReset();
    paneMocks.createPaneProtocolId.mockImplementation((prefix) => `${prefix}-${++nextPaneId}`);
    paneMocks.createTauriPaneReplication.mockImplementation((options) => {
      const replication: FakePaneReplication = {
        options,
        dispose: vi.fn<() => void>(),
        publishAuthoritativeState: vi.fn<(state: unknown) => void>(),
        publishEditorContent: vi.fn<(content: string) => void>(),
        start: vi.fn<() => void>(),
      };
      paneMocks.instances.push(replication);
      return replication;
    });
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    currentSession = null;
    disableTauriRuntime();
    vi.useRealTimers();
  });

  it.each(['normal', 'recent'] as const)(
    'keeps a %s prepared open provisional and blocks edit, save, and save-as until commit',
    async (kind) => {
      const commit = deferred<OpenCommitResult>();
      const prepared = preparedOpen(`${kind}-prepared`);
      if (kind === 'normal') tauriMocks.openFileDialog.mockResolvedValueOnce(prepared);
      else tauriMocks.openRecentFile.mockResolvedValueOnce(prepared);
      tauriMocks.commitRecentOpen.mockReturnValueOnce(commit.promise);

      act(() => root.render(<SessionHarness />));
      const { completion: openPromise } = await beginOpen(kind);

      expect(documentSnapshot(session())).toMatchObject({
        activePath: prepared.file.path,
        authorityStatus: 'provisional',
        content: prepared.file.content,
        dirty: false,
      });

      await act(async () => {
        session().updateContent('# Escaped mutation');
        await Promise.all([
          session().handleSave(),
          session().handleSaveAs(),
          session().saveCurrentDocument(),
        ]);
      });

      expect(session().content).toBe(prepared.file.content);
      expect(tauriMocks.writeFile).not.toHaveBeenCalled();
      expect(tauriMocks.saveAsDialog).not.toHaveBeenCalled();

      commit.resolve(committed());
      await act(async () => openPromise);

      expect(session().authorityStatus).toBe('committed');
      act(() => session().updateContent('# Committed mutation'));
      expect(session().content).toBe('# Committed mutation');
      expect(tauriMocks.commitRecentOpen).toHaveBeenCalledOnce();
      expect(tauriMocks.openRecentFile.mock.calls).toEqual(
        kind === 'recent' ? [['recent-entry']] : [],
      );
    },
  );

  it('blocks editor-popout mutation publication while its authoritative snapshot is provisional', async () => {
    act(() => root.render(<SessionHarness isPopout popoutPane="editor" />));
    const replication = paneMocks.instances[0];
    if (!replication) throw new Error('Expected an editor-popout replication port');
    const provisional = provisionalPaneSnapshot();

    act(() => replication.options.observe(provisional));
    expect(session().authorityStatus).toBe('provisional');

    await act(async () => {
      session().updateContent('# Escaped popout mutation');
      await Promise.all([session().handleSave(), session().handleSaveAs()]);
    });

    expect(session().content).toBe('# Provisional');
    expect(replication.publishEditorContent).not.toHaveBeenCalled();
    expect(tauriMocks.writeFile).not.toHaveBeenCalled();
    expect(tauriMocks.saveAsDialog).not.toHaveBeenCalled();

    act(() => replication.options.observe({
      ...provisional,
      revision: 2,
      state: { ...provisional.state, authorityStatus: 'committed' },
    }));
    act(() => session().updateContent('# Committed popout mutation'));

    expect(session().content).toBe('# Committed popout mutation');
    expect(replication.publishEditorContent).toHaveBeenCalledWith('# Committed popout mutation');
  });

  it.each(['normal', 'recent'] as const)(
    'restores the complete dirty prior snapshot after a same-generation %s not_committed result',
    async (kind) => {
      const priorOpen = preparedOpen('prior-html', {
        kind: 'html',
        path: '/workspace/prior.html',
        content_mode: 'text',
        content: '<h1>Saved prior</h1>',
        mime_type: 'text/html',
      });
      tauriMocks.openFileDialog.mockResolvedValueOnce(priorOpen);

      act(() => root.render(<SessionHarness />));
      await act(async () => session().handleOpenFile());
      act(() => session().updateContent('<h1>Dirty prior</h1>'));
      const prior = documentSnapshot(session());
      expect(prior.dirty).toBe(true);

      const rejectedOpen = preparedOpen(`${kind}-rejected`);
      if (kind === 'normal') tauriMocks.openFileDialog.mockResolvedValueOnce(rejectedOpen);
      else tauriMocks.openRecentFile.mockResolvedValueOnce(rejectedOpen);
      tauriMocks.commitRecentOpen.mockResolvedValueOnce({
        status: 'not_committed',
        message: 'The prepared open could not be committed.',
      });

      await act(async () => {
        if (kind === 'normal') await session().handleOpenFile();
        else await session().handleOpenRecent('recent-rejected');
      });

      expect(documentSnapshot(session())).toEqual(prior);
      expect(session().error).toBe('The prepared open could not be committed.');
      expect(restoreDocumentSnapshotMock).toHaveBeenCalledOnce();
    },
  );

  it('falls back to failed authority and remains read-only when prior restoration throws', async () => {
    const rejectedOpen = preparedOpen('restore-failed');
    tauriMocks.openFileDialog.mockResolvedValueOnce(rejectedOpen);
    tauriMocks.commitRecentOpen.mockResolvedValueOnce({
      status: 'not_committed',
      message: 'The prepared open could not be committed.',
    });
    restoreDocumentSnapshotMock.mockImplementationOnce(() => {
      throw new Error('injected restoration failure');
    });

    act(() => root.render(<SessionHarness />));
    await act(async () => session().handleOpenFile());

    expect(documentSnapshot(session())).toMatchObject({
      activePath: rejectedOpen.file.path,
      authorityStatus: 'failed',
      content: rejectedOpen.file.content,
      dirty: false,
    });

    await act(async () => {
      session().updateContent('# Escaped failed-state mutation');
      await Promise.all([session().handleSave(), session().handleSaveAs()]);
    });

    expect(session().content).toBe(rejectedOpen.file.content);
    expect(tauriMocks.writeFile).not.toHaveBeenCalled();
    expect(tauriMocks.saveAsDialog).not.toHaveBeenCalled();
    expect(session().error).toBe('The prepared open could not be committed.');
  });

  it('stays provisional through pending reconciliation and becomes unknown/read-only when unresolved', async () => {
    vi.useFakeTimers();
    const prepared = preparedOpen('unknown-result');
    tauriMocks.openFileDialog.mockResolvedValueOnce(prepared);
    tauriMocks.commitRecentOpen.mockRejectedValueOnce(new Error('commit response lost'));
    tauriMocks.getOpenCommitStatus
      .mockResolvedValueOnce({ status: 'pending' })
      .mockResolvedValueOnce({ status: 'unknown' });

    act(() => root.render(<SessionHarness />));
    const { completion: openPromise } = await beginOpen('normal');

    expect(session().authorityStatus).toBe('provisional');
    expect(tauriMocks.getOpenCommitStatus).toHaveBeenCalledOnce();
    await act(async () => {
      session().updateContent('# Escaped pending mutation');
      await session().handleSaveAs();
    });
    expect(session().content).toBe(prepared.file.content);
    expect(tauriMocks.saveAsDialog).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
      await openPromise;
    });

    expect(session().authorityStatus).toBe('unknown');
    expect(session().error).toBe(
      'The file authorization result could not be confirmed. Open another file to continue.',
    );
    await act(async () => {
      session().updateContent('# Escaped unknown mutation');
      await Promise.all([session().handleSave(), session().handleSaveAs()]);
    });
    expect(session().content).toBe(prepared.file.content);
    expect(tauriMocks.writeFile).not.toHaveBeenCalled();
    expect(tauriMocks.saveAsDialog).not.toHaveBeenCalled();
  });

  it('does not let an older completion restore over a newer winning document generation', async () => {
    const commit = deferred<OpenCommitResult>();
    const olderOpen = preparedOpen('older-open');
    tauriMocks.openFileDialog.mockResolvedValueOnce(olderOpen);
    tauriMocks.commitRecentOpen.mockReturnValueOnce(commit.promise);

    act(() => root.render(<SessionHarness />));
    act(() => session().updateContent('# Dirty prior'));
    const { completion: olderPromise } = await beginOpen('normal');
    expect(session().authorityStatus).toBe('provisional');

    act(() => {
      session().handleNew();
      session().updateContent('# Newer winner');
    });
    const newer = documentSnapshot(session());
    expect(newer).toMatchObject({
      activePath: null,
      authorityStatus: 'committed',
      content: '# Newer winner',
      dirty: true,
    });

    commit.resolve({
      status: 'not_committed',
      message: 'The older open could not be committed.',
    });
    await act(async () => olderPromise);

    expect(documentSnapshot(session())).toEqual(newer);
    expect(session().error).toBeNull();
    expect(restoreDocumentSnapshotMock).not.toHaveBeenCalled();
  });

  it('restores the main workspace before committing its prepared active document', async () => {
    enableTauriRuntime();
    const restored = preparedOpen('restored');
    tauriMocks.restoreWorkspaceSession.mockResolvedValue({
      workspace: workspaceSnapshot([{
        kind: 'markdown',
        path: restored.file.path,
        relative_path: 'restored.md',
        name: 'restored.md',
      }]),
      active_file: restored,
    });

    act(() => root.render(<SessionHarness />));
    await flushSessionEffects();

    expect(session()).toMatchObject({
      workspaceRoot: '/workspace',
      activePath: '/workspace/restored.md',
      authorityStatus: 'committed',
      content: '# restored',
    });
    expect(tauriMocks.restoreWorkspaceSession).toHaveBeenCalledOnce();
    expect(tauriMocks.commitRecentOpen).toHaveBeenCalledWith(restored.open_receipt);
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      restored.file.path,
    );
  });

  it('keeps a restored workspace and silently clears an active file whose receipt cannot commit', async () => {
    enableTauriRuntime();
    const restored = preparedOpen('restore-rejected');
    tauriMocks.restoreWorkspaceSession.mockResolvedValue({
      workspace: workspaceSnapshot([{
        kind: 'markdown',
        path: restored.file.path,
        relative_path: 'restore-rejected.md',
        name: 'restore-rejected.md',
      }]),
      active_file: restored,
    });
    tauriMocks.commitRecentOpen.mockResolvedValueOnce({
      status: 'not_committed',
      message: 'The restored receipt could not be committed.',
    });

    act(() => root.render(<SessionHarness />));
    await flushSessionEffects();

    expect(session()).toMatchObject({
      workspaceRoot: '/workspace',
      activePath: null,
      authorityStatus: 'committed',
      error: null,
    });
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      null,
    );
  });

  it('settles a missing session to a blank main pane and does not restore or persist in popouts', async () => {
    enableTauriRuntime();
    tauriMocks.restoreWorkspaceSession.mockResolvedValue(null);

    act(() => root.render(<SessionHarness isPopout popoutPane="editor" />));
    await flushSessionEffects();

    expect(session()).toMatchObject({
      workspaceRoot: null,
      activePath: null,
      content: EMPTY_MARKDOWN,
    });
    expect(tauriMocks.restoreWorkspaceSession).not.toHaveBeenCalled();
    expect(tauriMocks.persistWorkspaceSession).not.toHaveBeenCalled();

    act(() => root.unmount());
    root = createRoot(container);
    currentSession = null;
    act(() => root.render(<SessionHarness />));
    await flushSessionEffects();

    expect(session()).toMatchObject({ workspaceRoot: null, activePath: null });
    expect(tauriMocks.restoreWorkspaceSession).toHaveBeenCalledOnce();
    expect(tauriMocks.persistWorkspaceSession).not.toHaveBeenCalled();
  });

  it('persists null for an active document outside the restored workspace', async () => {
    enableTauriRuntime();
    tauriMocks.restoreWorkspaceSession.mockResolvedValue({
      workspace: workspaceSnapshot([{
        kind: 'markdown',
        path: '/workspace/notes.md',
        relative_path: 'notes.md',
        name: 'notes.md',
      }]),
      active_file: null,
    });
    tauriMocks.openFileDialog.mockResolvedValue(preparedOpen('outside', {
      kind: 'markdown',
      path: '/outside/notes.md',
      content_mode: 'text',
      content: '# Outside',
    }));

    act(() => root.render(<SessionHarness />));
    await flushSessionEffects();
    await act(async () => session().handleOpenFile());
    await flushSessionEffects();

    expect(session().activePath).toBe('/outside/notes.md');
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      null,
    );
  });

  it('blocks user document actions until a pending session restore settles', async () => {
    enableTauriRuntime();
    const restore = deferred<Awaited<ReturnType<typeof import('../lib/tauriCommands').restoreWorkspaceSession>>>();
    tauriMocks.restoreWorkspaceSession.mockReturnValue(restore.promise);

    act(() => root.render(<SessionHarness />));
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => session().handleOpenDirectory());
    act(() => session().updateContent('# Typed before restore'));
    expect(tauriMocks.openDirectoryDialog).not.toHaveBeenCalled();
    expect(session().content).toBe(EMPTY_MARKDOWN);

    restore.resolve({
      workspace: workspaceSnapshot(),
      active_file: null,
    });
    await flushSessionEffects();

    expect(session().workspaceRoot).toBe('/workspace');
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      null,
    );
  });

  it('restores only once when StrictMode replays effects', async () => {
    enableTauriRuntime();
    tauriMocks.restoreWorkspaceSession.mockResolvedValue({
      workspace: workspaceSnapshot(),
      active_file: null,
    });

    act(() => root.render(
      <StrictMode>
        <SessionHarness />
      </StrictMode>,
    ));
    await flushSessionEffects();

    expect(tauriMocks.restoreWorkspaceSession).toHaveBeenCalledOnce();
    expect(session().workspaceRoot).toBe('/workspace');
  });

  it('serializes workspace persistence so the newest committed file wins', async () => {
    enableTauriRuntime();
    const firstPersist = deferred<void>();
    const opened = preparedOpen('notes');
    tauriMocks.restoreWorkspaceSession.mockResolvedValue({
      workspace: workspaceSnapshot([{
        kind: 'markdown',
        path: opened.file.path,
        relative_path: 'notes.md',
        name: 'notes.md',
      }]),
      active_file: null,
    });
    tauriMocks.persistWorkspaceSession
      .mockReturnValueOnce(firstPersist.promise)
      .mockResolvedValueOnce(undefined);
    tauriMocks.openWorkspaceFile.mockResolvedValue(opened);

    act(() => root.render(<SessionHarness />));
    await flushSessionEffects();
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenCalledOnce();
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      null,
    );

    await act(async () => session().openWorkspaceFilePath(opened.file.path));
    await flushSessionEffects();
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenCalledOnce();

    firstPersist.resolve(undefined);
    await flushSessionEffects();
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenCalledTimes(2);
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      opened.file.path,
    );
  });

  it('flushes the latest committed workspace snapshot before the program closes', async () => {
    enableTauriRuntime();
    const firstPersist = deferred<void>();
    const opened = preparedOpen('closing');
    tauriMocks.restoreWorkspaceSession.mockResolvedValue({
      workspace: workspaceSnapshot([{
        kind: 'markdown',
        path: opened.file.path,
        relative_path: 'closing.md',
        name: 'closing.md',
      }]),
      active_file: null,
    });
    tauriMocks.persistWorkspaceSession
      .mockReturnValueOnce(firstPersist.promise)
      .mockResolvedValue(undefined);
    tauriMocks.openWorkspaceFile.mockResolvedValue(opened);

    act(() => root.render(<SessionHarness />));
    await flushSessionEffects();
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      null,
    );

    await act(async () => session().openWorkspaceFilePath(opened.file.path));
    let flush!: Promise<void>;
    await act(async () => {
      flush = session().flushWorkspaceSession();
      await Promise.resolve();
    });
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenCalledOnce();

    firstPersist.resolve(undefined);
    await act(async () => flush);

    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      opened.file.path,
    );
  });

  it('keeps the previously committed workspace file while a new prepared open is pending', async () => {
    enableTauriRuntime();
    const previous = preparedOpen('previous');
    const commit = deferred<OpenCommitResult>();
    const pending = preparedOpen('pending');
    tauriMocks.restoreWorkspaceSession.mockResolvedValue({
      workspace: workspaceSnapshot([
        {
          kind: 'markdown',
          path: previous.file.path,
          relative_path: 'previous.md',
          name: 'previous.md',
        },
        {
          kind: 'markdown',
          path: pending.file.path,
          relative_path: 'pending.md',
          name: 'pending.md',
        },
      ]),
      active_file: previous,
    });
    tauriMocks.openWorkspaceFile.mockResolvedValue(pending);

    act(() => root.render(<SessionHarness />));
    await flushSessionEffects();

    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      previous.file.path,
    );
    const persistedBeforePendingOpen = tauriMocks.persistWorkspaceSession.mock.calls.length;
    tauriMocks.commitRecentOpen.mockReturnValueOnce(commit.promise);

    let completion!: Promise<void>;
    await act(async () => {
      completion = session().openWorkspaceFilePath(pending.file.path);
      await Promise.resolve();
      await Promise.resolve();
    });
    await flushSessionEffects();

    expect(session().authorityStatus).toBe('provisional');
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenCalledTimes(persistedBeforePendingOpen);
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      previous.file.path,
    );

    commit.resolve(committed());
    await act(async () => completion);
    await flushSessionEffects();

    expect(session().authorityStatus).toBe('committed');
    expect(tauriMocks.persistWorkspaceSession).toHaveBeenLastCalledWith(
      'restored-workspace-token',
      '/workspace',
      pending.file.path,
    );
  });
});
