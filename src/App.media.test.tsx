// @vitest-environment jsdom

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';

const appMocks = vi.hoisted(() => ({
  docxPreview: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  editorPane: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  excalidrawPane: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  emit: vi.fn<(event: string, payload: unknown) => Promise<void>>(),
  fileSidebar: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  jinxiuMarkdown: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  listen: vi.fn<(event: string, handler: (event: { payload: unknown }) => void) => Promise<() => void>>(),
  paneResizer: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  pdfPreview: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  previewPane: vi.fn<(props: Record<string, unknown>) => null>(() => null),
  setNativeSaveMenuEnabled: vi.fn<(enabled: boolean) => Promise<void>>(),
  useDocumentSession: vi.fn<(input: Record<string, unknown>) => Record<string, unknown>>(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  emit: appMocks.emit,
  listen: appMocks.listen,
}));
vi.mock('./components/EditorPane', () => ({ EditorPane: appMocks.editorPane }));
vi.mock('./components/DocxPreview', () => ({ DocxPreview: appMocks.docxPreview }));
vi.mock('./components/ExcalidrawPane', () => ({ ExcalidrawPane: appMocks.excalidrawPane }));
vi.mock('./components/FileSidebar', () => ({ FileSidebar: appMocks.fileSidebar }));
vi.mock('./components/JinxiuMarkdown', () => ({ default: appMocks.jinxiuMarkdown }));
vi.mock('./components/PaneResizer', () => ({ PaneResizer: appMocks.paneResizer }));
vi.mock('./components/PdfPreview', () => ({ PdfPreview: appMocks.pdfPreview }));
vi.mock('./components/PreviewPane', () => ({ PreviewPane: appMocks.previewPane }));
vi.mock('./hooks/useDocumentSession', () => ({
  useDocumentSession: appMocks.useDocumentSession,
}));
vi.mock('./hooks/usePanePopouts', () => ({
  usePanePopouts: () => ({
    closePopoutWindows: vi.fn<() => Promise<void>>(async () => undefined),
    editorPopoutButton: undefined,
    openPanePopout: vi.fn<(pane: 'editor' | 'preview') => Promise<void>>(async () => undefined),
    previewPopoutButton: undefined,
  }),
}));
vi.mock('./hooks/usePaneResize', () => ({
  usePaneResize: () => ({
    editorPaneRef: { current: null },
    movePaneResize: vi.fn<() => void>(),
    previewPaneRef: { current: null },
    startPaneResize: vi.fn<() => void>(),
    stopPaneResize: vi.fn<() => void>(),
  }),
}));
vi.mock('./hooks/useProgramCloseGuard', () => ({
  useProgramCloseGuard: () => ({
    forceCloseProgram: vi.fn<() => Promise<void>>(async () => undefined),
  }),
}));
vi.mock('./lib/tauriCommands', () => ({
  setNativeSaveMenuEnabled: appMocks.setNativeSaveMenuEnabled,
}));

function createMarkdownSession() {
  return {
    activeFileKind: 'markdown',
    activeMimeType: null,
    activePath: '/workspace/docs/guide.md',
    authorityStatus: 'committed',
    broadcastPaneState: vi.fn<() => Promise<void>>(async () => undefined),
    busy: false,
    bytesBase64: null,
    content: '# Guide',
    createFileInWorkspace: vi.fn<() => Promise<void>>(async () => undefined),
    createFolderInWorkspace: vi.fn<() => Promise<void>>(async () => undefined),
    deleteWorkspaceEntryPath: vi.fn<() => Promise<void>>(async () => undefined),
    dirty: false,
    documentEpoch: 1,
    documentId: 'document-guide',
    error: null,
    externalFileAction: null,
    files: [{
      kind: 'markdown',
      name: 'guide.md',
      path: '/workspace/docs/guide.md',
      relative_path: 'docs/guide.md',
    }],
    fileTree: [],
    handleClearRecent: vi.fn<() => Promise<void>>(async () => undefined),
    handleCloseDeletedDraft: vi.fn<() => Promise<void>>(async () => undefined),
    handleKeepCurrentExternal: vi.fn<() => Promise<void>>(async () => undefined),
    handleNew: vi.fn<() => void>(),
    handleOpenDirectory: vi.fn<() => Promise<void>>(async () => undefined),
    handleOpenFile: vi.fn<() => Promise<void>>(async () => undefined),
    handleOpenRecent: vi.fn<(entryId: string) => Promise<void>>(async () => undefined),
    handleSave: vi.fn<() => Promise<void>>(async () => undefined),
    handleSaveAs: vi.fn<() => Promise<void>>(async () => undefined),
    handleSaveDeletedDraftAs: vi.fn<() => Promise<void>>(async () => undefined),
    handleUseExternal: vi.fn<() => Promise<void>>(async () => undefined),
    moveWorkspaceEntryPath: vi.fn<() => Promise<void>>(async () => undefined),
    notice: null,
    openWorkspaceFilePath: vi.fn<() => Promise<void>>(async () => undefined),
    previewRevision: 0,
    refreshWorkspace: vi.fn<() => Promise<void>>(async () => undefined),
    renameWorkspaceEntryPath: vi.fn<() => Promise<void>>(async () => undefined),
    saveCurrentDocument: vi.fn<() => Promise<boolean>>(async () => true),
    setError: vi.fn<(message: string | null) => void>(),
    setNotice: vi.fn<(message: string | null) => void>(),
    updateContent: vi.fn<(content: string) => void>(),
    workspaceRoot: '/workspace',
  };
}

describe('App workspace media insertion', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    appMocks.editorPane.mockClear();
    appMocks.emit.mockReset();
    appMocks.emit.mockResolvedValue(undefined);
    appMocks.fileSidebar.mockClear();
    appMocks.jinxiuMarkdown.mockClear();
    appMocks.listen.mockReset();
    appMocks.listen.mockResolvedValue(() => undefined);
    appMocks.paneResizer.mockClear();
    appMocks.previewPane.mockClear();
    appMocks.setNativeSaveMenuEnabled.mockReset();
    appMocks.setNativeSaveMenuEnabled.mockResolvedValue(undefined);
    appMocks.useDocumentSession.mockReset();
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('routes a dropped workspace image into the current Markdown editor', async () => {
    appMocks.useDocumentSession.mockReturnValue(createMarkdownSession());
    await act(async () => root.render(<App />));

    const onInsertWorkspaceAsset = appMocks.fileSidebar.mock.lastCall?.[0].onInsertWorkspaceAsset as
      | ((asset: Record<string, unknown>, position: { clientX: number; clientY: number }) => void)
      | undefined;
    expect(onInsertWorkspaceAsset).toEqual(expect.any(Function));

    act(() => onInsertWorkspaceAsset?.({
      kind: 'image',
      name: 'cover.png',
      path: '/workspace/assets/cover.png',
      relative_path: 'assets/cover.png',
    }, { clientX: 240, clientY: 160 }));

    expect(appMocks.editorPane.mock.lastCall?.[0].mediaInsertion).toEqual({
      clientX: 240,
      clientY: 160,
      documentEpoch: 1,
      documentId: 'document-guide',
      markdown: '![cover.png](../assets/cover.png)',
      requestId: 1,
    });
  });

  it('routes a dropped workspace audio file as a Markdown link', async () => {
    appMocks.useDocumentSession.mockReturnValue(createMarkdownSession());
    await act(async () => root.render(<App />));

    const onInsertWorkspaceAsset = appMocks.fileSidebar.mock.lastCall?.[0].onInsertWorkspaceAsset as
      | ((asset: Record<string, unknown>, position: { clientX: number; clientY: number }) => void)
      | undefined;
    act(() => onInsertWorkspaceAsset?.({
      kind: 'audio',
      name: 'intro.mp3',
      path: '/workspace/audio/intro.mp3',
      relative_path: 'audio/intro.mp3',
    }, { clientX: 240, clientY: 160 }));

    expect(appMocks.editorPane.mock.lastCall?.[0].mediaInsertion).toMatchObject({
      markdown: '[intro.mp3](../audio/intro.mp3)',
    });
  });
});
