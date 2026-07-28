// @vitest-environment jsdom

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  ActiveDocumentWatchEvent,
  ActiveDocumentWatchRegistration,
  ActiveDocumentWatchSnapshotEnvelope,
  ActiveDocumentWatchTransport,
} from '../lib/activeDocumentWatch';
import type { PaneSnapshotEnvelope } from '../lib/paneSync';
import type { PreparedOpenFileResponse } from '../types';
import { useDocumentSession } from './useDocumentSession';

interface MockPaneReplicationOptions {
  observe: (snapshot: PaneSnapshotEnvelope) => void;
  role: 'main' | 'editor-popout' | 'preview-popout';
}

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
  nextId: 0,
  createPaneProtocolId: vi.fn<(prefix: string) => string>(),
  createTauriPaneReplication: vi.fn<(options: MockPaneReplicationOptions) => {
    options: MockPaneReplicationOptions;
    dispose: () => void;
    publishAuthoritativeState: (state: unknown) => void;
    publishEditorContent: (content: string) => void;
    start: () => void;
  }>((options) => ({
    options,
    dispose: vi.fn<() => void>(),
    publishAuthoritativeState: vi.fn<(state: unknown) => void>(),
    publishEditorContent: vi.fn<(content: string) => void>(),
    start: vi.fn<() => void>(),
  })),
}));

vi.mock('../lib/tauriCommands', () => tauriMocks);
vi.mock('../lib/tauriPaneReplication', () => ({
  createPaneProtocolId: paneMocks.createPaneProtocolId,
  createTauriPaneReplication: paneMocks.createTauriPaneReplication,
}));

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

class FakeActiveDocumentWatchTransport implements ActiveDocumentWatchTransport {
  activate = vi.fn<ActiveDocumentWatchTransport['activate']>(async () => true);
  reconcile = vi.fn<ActiveDocumentWatchTransport['reconcile']>();
  stop = vi.fn<ActiveDocumentWatchTransport['stop']>(async () => true);
  startCalls: Array<{ path: string; documentId: string; documentGeneration: number }> = [];
  listener: ((event: ActiveDocumentWatchEvent) => void) | null = null;
  nextWatch = 1;

  async start(
    path: string,
    documentId: string,
    documentGeneration: number,
  ): Promise<ActiveDocumentWatchRegistration> {
    this.startCalls.push({ path, documentId, documentGeneration });
    return {
      protocol_version: 1,
      watch_id: `watch-${this.nextWatch++}`,
      document_id: documentId,
      document_generation: documentGeneration,
      sequence: 1,
      snapshot: {
        status: 'present',
        file: {
          kind: 'markdown',
          path,
          content_mode: 'text',
          content: '# Saved',
        },
        preview_revision: 1,
      },
    };
  }

  async listen(callback: (event: ActiveDocumentWatchEvent) => void) {
    this.listener = callback;
    return () => {
      if (this.listener === callback) this.listener = null;
    };
  }

  emit(event: ActiveDocumentWatchEvent) {
    this.listener?.(event);
  }
}

type Session = ReturnType<typeof useDocumentSession>;
let currentSession: Session | null = null;

function Harness({
  isPopout = false,
  popoutPane = 'main',
  transport,
}: {
  isPopout?: boolean;
  popoutPane?: 'main' | 'editor' | 'preview';
  transport: ActiveDocumentWatchTransport;
}) {
  currentSession = useDocumentSession({
    activeDocumentWatchTransport: transport,
    isPopout,
    popoutPane,
  });
  return null;
}

function session(): Session {
  if (!currentSession) throw new Error('Expected a mounted document session');
  return currentSession;
}

function preparedOpen(): PreparedOpenFileResponse {
  return {
    file: {
      kind: 'markdown',
      path: '/workspace/notes.md',
      content_mode: 'text',
      content: '# Saved',
    },
    open_receipt: 'open-receipt',
    commit_operation_id: 'commit-operation',
  };
}

async function flushEffects() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function openDocument() {
  await act(async () => session().handleOpenFile());
  await flushEffects();
}

function stateEvent(
  watchId: string,
  documentId: string,
  documentGeneration: number,
  sequence: number,
  content: string,
): ActiveDocumentWatchEvent {
  return {
    protocol_version: 1,
    watch_id: watchId,
    document_id: documentId,
    document_generation: documentGeneration,
    sequence,
    event: {
      kind: 'state',
      reason: 'changed',
      previous_path: null,
      snapshot: {
        status: 'present',
        file: {
          kind: 'markdown',
          path: '/workspace/notes.md',
          content_mode: 'text',
          content,
        },
        preview_revision: sequence,
      },
    },
  };
}

describe('useDocumentSession active document monitoring', () => {
  let container: HTMLDivElement;
  let root: Root;
  let transport: FakeActiveDocumentWatchTransport;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    currentSession = null;
    transport = new FakeActiveDocumentWatchTransport();
    paneMocks.nextId = 0;
    paneMocks.createPaneProtocolId.mockReset();
    paneMocks.createPaneProtocolId.mockImplementation((prefix) => `${prefix}-${++paneMocks.nextId}`);
    paneMocks.createTauriPaneReplication.mockClear();
    for (const mock of Object.values(tauriMocks)) mock.mockReset();
    tauriMocks.openFileDialog.mockResolvedValue(preparedOpen());
    tauriMocks.persistWorkspaceSession.mockResolvedValue(undefined);
    tauriMocks.restoreWorkspaceSession.mockResolvedValue(null);
    tauriMocks.commitRecentOpen.mockResolvedValue({
      status: 'committed',
      recent_files: { entries: [] },
    });
    tauriMocks.getOpenCommitStatus.mockResolvedValue({ status: 'unknown' });
    tauriMocks.discardOpenReceipt.mockResolvedValue(true);
    tauriMocks.saveAsDialog.mockResolvedValue(null);
    tauriMocks.writeFile.mockResolvedValue(undefined);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    currentSession = null;
  });

  it('starts, applies, and activates only from the main session', async () => {
    act(() => root.render(<Harness transport={transport} />));
    await openDocument();

    expect(transport.startCalls).toHaveLength(1);
    expect(transport.startCalls[0]).toMatchObject({
      path: '/workspace/notes.md',
      documentId: session().documentId,
      documentGeneration: 1,
    });
    expect(transport.activate).toHaveBeenCalledWith(
      'watch-1',
      session().documentId,
      1,
      1,
    );

    act(() => root.unmount());
    expect(transport.stop).toHaveBeenCalledWith('watch-1');

    root = createRoot(container);
    currentSession = null;
    const popoutTransport = new FakeActiveDocumentWatchTransport();
    act(() => root.render(
      <Harness isPopout popoutPane="preview" transport={popoutTransport} />,
    ));
    await flushEffects();
    expect(popoutTransport.startCalls).toHaveLength(0);
    expect(popoutTransport.listener).toBeNull();
  });

  it('silently reloads a clean external change and advances the document epoch', async () => {
    act(() => root.render(<Harness transport={transport} />));
    await openDocument();
    const beforeEpoch = session().documentEpoch;
    const call = transport.startCalls[0]!;

    act(() => transport.emit(stateEvent(
      'watch-1',
      call.documentId,
      call.documentGeneration,
      2,
      '# External',
    )));
    await flushEffects();

    expect(session().content).toBe('# External');
    expect(session().lastSavedContent).toBe('# External');
    expect(session().documentEpoch).toBe(beforeEpoch + 1);
    expect(session().externalFileAction).toBeNull();
    expect(paneMocks.createTauriPaneReplication.mock.results[0]?.value.publishAuthoritativeState)
      .toHaveBeenLastCalledWith(expect.objectContaining({
        documentEpoch: beforeEpoch + 1,
        previewRevision: 2,
      }));
  });

  it('coalesces a dirty conflict and Keep Current uses apply-time editor content', async () => {
    const reconcile = deferred<ActiveDocumentWatchSnapshotEnvelope>();
    transport.reconcile.mockReturnValue(reconcile.promise);
    act(() => root.render(<Harness transport={transport} />));
    await openDocument();
    const call = transport.startCalls[0]!;
    act(() => session().updateContent('# Dirty'));

    act(() => {
      transport.emit(stateEvent('watch-1', call.documentId, call.documentGeneration, 2, '# External 1'));
      transport.emit(stateEvent('watch-1', call.documentId, call.documentGeneration, 3, '# External 2'));
    });
    await flushEffects();
    expect(session().externalFileAction).toMatchObject({ kind: 'conflict' });
    expect(session().content).toBe('# Dirty');

    let completion!: Promise<void>;
    await act(async () => {
      completion = session().handleKeepCurrentExternal();
      await Promise.resolve();
    });
    act(() => session().updateContent('# Edited while waiting'));
    reconcile.resolve({
      protocol_version: 1,
      watch_id: 'watch-1',
      document_id: call.documentId,
      document_generation: call.documentGeneration,
      sequence: 4,
      reason: 'resync',
      previous_path: null,
      snapshot: {
        status: 'present',
        file: {
          kind: 'markdown',
          path: '/workspace/notes.md',
          content_mode: 'text',
          content: '# Latest external',
        },
        preview_revision: 4,
      },
    });
    await act(async () => completion);

    expect(session().content).toBe('# Edited while waiting');
    expect(session().lastSavedContent).toBe('# Latest external');
    expect(session().dirty).toBe(true);
    expect(session().externalFileAction).toBeNull();
    expect(tauriMocks.writeFile).not.toHaveBeenCalled();
  });

  it('preserves a dirty deleted draft through Save As cancellation and closes explicitly', async () => {
    act(() => root.render(<Harness transport={transport} />));
    await openDocument();
    const call = transport.startCalls[0]!;
    act(() => session().updateContent('# Unsaved draft'));

    act(() => transport.emit({
      protocol_version: 1,
      watch_id: 'watch-1',
      document_id: call.documentId,
      document_generation: call.documentGeneration,
      sequence: 2,
      event: {
        kind: 'state',
        reason: 'missing',
        previous_path: null,
        snapshot: { status: 'missing', path: '/workspace/notes.md' },
      },
    }));
    await flushEffects();

    expect(session().externalFileAction).toMatchObject({ kind: 'deleted-draft' });
    expect(session().content).toBe('# Unsaved draft');
    expect(transport.stop).toHaveBeenCalledWith('watch-1');

    await act(async () => session().handleSaveDeletedDraftAs());
    expect(session().externalFileAction).toMatchObject({ kind: 'deleted-draft' });
    expect(session().content).toBe('# Unsaved draft');

    await act(async () => session().handleCloseDeletedDraft());
    expect(session().externalFileAction).toBeNull();
    expect(session().activePath).toBeNull();
  });

  it('refreshes the active workspace after saving a deleted draft under a new name', async () => {
    tauriMocks.openDirectoryDialog.mockResolvedValue({
      root: '/workspace',
      workspace_token: 'workspace-token',
      files: [{
        kind: 'markdown',
        path: '/workspace/notes.md',
        relative_path: 'notes.md',
        name: 'notes.md',
      }],
      directories: [],
    });
    tauriMocks.saveAsDialog.mockResolvedValue({
      status: 'confirmed-committed',
      receipt: {
        committed: { path: '/workspace/recovered.md' },
        workspace: { status: 'not-applicable' },
      },
    });
    tauriMocks.refreshDirectory.mockResolvedValue({
      root: '/workspace',
      workspace_token: 'workspace-token',
      files: [{
        kind: 'markdown',
        path: '/workspace/recovered.md',
        relative_path: 'recovered.md',
        name: 'recovered.md',
      }],
      directories: [],
    });

    act(() => root.render(<Harness transport={transport} />));
    await act(async () => session().handleOpenDirectory());
    await openDocument();
    const call = transport.startCalls[0]!;
    act(() => session().updateContent('# Unsaved draft'));
    act(() => transport.emit({
      protocol_version: 1,
      watch_id: 'watch-1',
      document_id: call.documentId,
      document_generation: call.documentGeneration,
      sequence: 2,
      event: {
        kind: 'state',
        reason: 'missing',
        previous_path: null,
        snapshot: { status: 'missing', path: '/workspace/notes.md' },
      },
    }));
    await flushEffects();

    await act(async () => session().handleSaveDeletedDraftAs());

    expect(tauriMocks.refreshDirectory).toHaveBeenCalledWith(
      'workspace-token',
      '/workspace',
    );
    expect(session().files).toEqual([expect.objectContaining({
      path: '/workspace/recovered.md',
    })]);
    expect(session().activePath).toBe('/workspace/recovered.md');
    expect(session().externalFileAction).toBeNull();
  });

  it('rejects stale sequence events after the registration barrier', async () => {
    act(() => root.render(<Harness transport={transport} />));
    await openDocument();
    const call = transport.startCalls[0]!;

    act(() => transport.emit(stateEvent(
      'watch-1',
      call.documentId,
      call.documentGeneration,
      1,
      '# Stale',
    )));
    await flushEffects();

    expect(session().content).toBe('# Saved');
    expect(session().externalFileAction).toBeNull();
  });
});
