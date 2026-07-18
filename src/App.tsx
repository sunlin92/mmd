import {
  lazy,
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentProps,
} from 'react';
import { emit, listen } from '@tauri-apps/api/event';
import { AppToolbar } from './components/AppToolbar';
import type { DocxPreviewFeedback } from './components/DocxPreview';
import { EditorPane } from './components/EditorPane';
import { ExternalFileChangeDialog } from './components/ExternalFileChangeDialog';
import { FeedbackDialog } from './components/FeedbackDialog';
import { FileSidebar } from './components/FileSidebar';
import JinxiuMarkdown from './components/JinxiuMarkdown';
import { LazyPreviewBoundary } from './components/LazyPreviewBoundary';
import { PaneResizer } from './components/PaneResizer';
import type { PdfPreviewFeedback } from './components/PdfPreview';
import { PopoutPaneShell } from './components/PopoutPaneShell';
import { PreviewPane } from './components/PreviewPane';
import { UnsavedExitDialog } from './components/UnsavedExitDialog';
import { WorkspaceEntryDialog, type WorkspaceEntryOperation } from './components/WorkspaceEntryDialog';
import { WorkspaceImagePreview } from './components/WorkspaceImagePreview';
import { WorkspaceHtmlPreview } from './components/WorkspaceHtmlPreview';
import { WorkspaceMediaPreview } from './components/WorkspaceMediaPreview';
import { WorkspaceMoveDialog, type WorkspaceMoveOperation } from './components/WorkspaceMoveDialog';
import { WorkspaceSidebarResizer } from './components/WorkspaceSidebarResizer';
import { useDocumentSession } from './hooks/useDocumentSession';
import { usePanePopouts } from './hooks/usePanePopouts';
import { usePaneResize } from './hooks/usePaneResize';
import { useProgramCloseGuard } from './hooks/useProgramCloseGuard';
import { useI18n } from './lib/i18n';
import type { EffectiveLocale } from './lib/locale';
import { useWorkspaceSidebarResize } from './hooks/useWorkspaceSidebarResize';
import { APP_FEEDBACK_ERROR_EVENT, getFeedbackDialog, normalizeAppError } from './lib/appFeedback';
import { getUnsavedExitPrompt, getUnsavedFileSwitchPrompt } from './lib/closeGuard';
import {
  decodeNativeMenuCommand,
  isNativeSaveMenuEnabled,
  NATIVE_MENU_EVENT,
} from './lib/nativeMenu';
import { getPaneLayoutStyle, parsePopoutPane } from './lib/paneLayout';
import {
  decodeMarkdownOutlineJump,
  extractMarkdownOutline,
  OUTLINE_JUMP_EVENT,
  type MarkdownOutlineItem,
  type MarkdownOutlineJump,
} from './lib/markdownOutline';
import {
  createMarkdownMediaReference,
  type MarkdownMediaInsertion,
} from './lib/markdownMedia';
import {
  DEFAULT_WORKSPACE_SIDEBAR_WIDTH,
  getWorkspaceLayoutClassName,
  getWorkspaceSidebarLayoutStyle,
} from './lib/sidebarLayout';
import { setNativeSaveMenuEnabled } from './lib/tauriCommands';
import { getWorkspaceMoveDestinations } from './lib/fileTreeOperations';
import { getWorkspacePresentation } from './lib/workspaceFileKind';
import type { WorkspaceFileEntry } from './types';
import './styles.css';

const LazyDocxPreview = lazy(() => import('./components/DocxPreview').then((module) => ({
  default: module.DocxPreview,
})));
const LazyExcalidrawPane = lazy(() => import('./components/ExcalidrawPane').then((module) => ({
  default: module.ExcalidrawPane,
})));
const LazyPdfPreview = lazy(() => import('./components/PdfPreview').then((module) => ({
  default: module.PdfPreview,
})));

interface LazyPreviewWrapperProps {
  loadingLabel: string;
  locale: EffectiveLocale;
}

function DocxPreview({ loadingLabel, locale, ...props }:
ComponentProps<typeof LazyDocxPreview> & LazyPreviewWrapperProps) {
  return (
    <LazyPreviewBoundary loadingLabel={loadingLabel} locale={locale}>
      <LazyDocxPreview {...props} />
    </LazyPreviewBoundary>
  );
}

function ExcalidrawPane({ loadingLabel, locale, ...props }:
ComponentProps<typeof LazyExcalidrawPane> & LazyPreviewWrapperProps) {
  return (
    <LazyPreviewBoundary loadingLabel={loadingLabel} locale={locale}>
      <LazyExcalidrawPane {...props} />
    </LazyPreviewBoundary>
  );
}

function PdfPreview({ loadingLabel, locale, ...props }:
ComponentProps<typeof LazyPdfPreview> & LazyPreviewWrapperProps) {
  return (
    <LazyPreviewBoundary loadingLabel={loadingLabel} locale={locale}>
      <LazyPdfPreview {...props} />
    </LazyPreviewBoundary>
  );
}

function currentPopoutPane() {
  return typeof window === 'undefined' ? 'main' : parsePopoutPane(window.location.search);
}

export default function App() {
  const { locale, t } = useI18n();
  const popoutPane = useMemo(() => currentPopoutPane(), []);
  const isPopout = popoutPane !== 'main';
  const [showUnsavedExitPrompt, setShowUnsavedExitPrompt] = useState(false);
  const [pendingFileSwitchPath, setPendingFileSwitchPath] = useState<string | null>(null);
  const [workspaceEntryOperation, setWorkspaceEntryOperation] = useState<WorkspaceEntryOperation | null>(null);
  const [workspaceMoveOperation, setWorkspaceMoveOperation] = useState<WorkspaceMoveOperation | null>(null);
  const [collapsedFolders, setCollapsedFolders] = useState<Set<string>>(() => new Set());
  const [fileTreeCollapsed, setFileTreeCollapsed] = useState(false);
  const [editorPaneRatio, setEditorPaneRatio] = useState(0.5);
  const [workspaceSidebarWidth, setWorkspaceSidebarWidth] = useState(DEFAULT_WORKSPACE_SIDEBAR_WIDTH);
  const [outlineJump, setOutlineJump] = useState<MarkdownOutlineJump | null>(null);
  const [mediaInsertion, setMediaInsertion] = useState<MarkdownMediaInsertion | null>(null);
  const nativeSaveMenuSyncRef = useRef<Promise<void>>(Promise.resolve());
  const outlineJumpRequestIdRef = useRef(0);
  const mediaInsertionRequestIdRef = useRef(0);
  const paneLayoutStyle = useMemo(() => getPaneLayoutStyle(editorPaneRatio), [editorPaneRatio]);

  const {
    activeFileKind,
    activeMimeType,
    activePath,
    authorityStatus,
    broadcastPaneState,
    busy,
    bytesBase64,
    content,
    createFileInWorkspace,
    createFolderInWorkspace,
    deleteWorkspaceEntryPath,
    dirty,
    documentEpoch,
    documentId,
    error,
    externalFileAction,
    files = [],
    fileTree,
    flushWorkspaceSession,
    handleNew,
    handleOpenDirectory,
    handleOpenFile,
    handleOpenRecent,
    handleClearRecent,
    handleCloseDeletedDraft,
    handleKeepCurrentExternal,
    handleSave,
    handleSaveAs,
    handleSaveDeletedDraftAs,
    handleUseExternal,
    moveWorkspaceEntryPath,
    notice,
    openWorkspaceFilePath,
    previewRevision,
    renameWorkspaceEntryPath,
    refreshWorkspace,
    saveCurrentDocument,
    setError,
    setNotice,
    updateContent,
    workspaceRoot,
  } = useDocumentSession({ isPopout, popoutPane });

  const feedbackDialog = useMemo(() => getFeedbackDialog({ error, notice }, locale), [error, locale, notice]);
  const unsavedExitPrompt = useMemo(() => getUnsavedExitPrompt(activePath, locale), [activePath, locale]);
  const unsavedFileSwitchPrompt = useMemo(() => (
    pendingFileSwitchPath
      ? getUnsavedFileSwitchPrompt(activePath, pendingFileSwitchPath, locale)
      : null
  ), [activePath, locale, pendingFileSwitchPath]);
  const activePresentation = getWorkspacePresentation(activeFileKind);
  const workspaceLayoutStyle = {
    ...getWorkspaceSidebarLayoutStyle(workspaceSidebarWidth),
    ...('editor' in activePresentation ? paneLayoutStyle : {}),
  };
  const deferredOutlineContent = useDeferredValue(content);
  const outlineItems = useMemo(() => (
    activeFileKind === 'markdown' ? extractMarkdownOutline(deferredOutlineContent) : []
  ), [activeFileKind, deferredOutlineContent]);
  const currentOutlineJump = outlineJump?.documentId === documentId
    && outlineJump.documentEpoch === documentEpoch
    ? outlineJump
    : null;
  const currentMediaInsertion = mediaInsertion?.documentId === documentId
    && mediaInsertion.documentEpoch === documentEpoch
    ? mediaInsertion
    : null;
  const activeWorkspaceMarkdownFile = useMemo(() => (
    activeFileKind === 'markdown'
      ? files.find((file) => file.path === activePath && file.kind === 'markdown') ?? null
      : null
  ), [activeFileKind, activePath, files]);
  const editorFileKind = 'editor' in activePresentation ? activePresentation.editor : 'markdown';
  const isImageFile = activePresentation.preview === 'image';
  const isMediaFile = activePresentation.preview === 'media';
  const isPdfFile = activePresentation.preview === 'pdf';
  const isDocxFile = activePresentation.preview === 'docx';
  const isExcalidrawFile = activePresentation.preview === 'excalidraw';
  const isDocumentFile = isPdfFile || isDocxFile;
  const mediaKind = isMediaFile ? activePresentation.media_kind : 'video';
  const mediaMimeType = activeMimeType ?? (mediaKind === 'audio' ? 'audio/*' : 'video/*');
  const documentAssetsEnabled = authorityStatus === 'committed';
  const workspaceMoveDestinations = useMemo(() => {
    if (!workspaceMoveOperation || !workspaceRoot) return [];
    return getWorkspaceMoveDestinations({
      fileTree,
      sourceKind: workspaceMoveOperation.entryKind,
      sourcePath: workspaceMoveOperation.path,
      workspaceRoot,
    });
  }, [fileTree, workspaceMoveOperation, workspaceRoot]);
  const nativeSaveMenuEnabled = isNativeSaveMenuEnabled({
    authorityStatus,
    busy: busy || externalFileAction !== null,
    fileKind: activeFileKind,
  });

  const handleDocumentPreviewFeedback = useCallback((
    feedback: DocxPreviewFeedback | PdfPreviewFeedback,
  ) => {
    if (feedback.kind === 'error') {
      setError(feedback.message);
      setNotice(null);
    } else {
      setNotice(feedback.message);
      setError(null);
    }
  }, [setError, setNotice]);

  const handleExcalidrawError = useCallback((message: string) => {
    setError(message);
    setNotice(null);
  }, [setError, setNotice]);

  const documentPreview = isPdfFile
    ? (
      <PdfPreview
        bytesBase64={bytesBase64}
        documentEpoch={documentEpoch}
        documentId={documentId}
        enabled={documentAssetsEnabled}
        loadingLabel={t('loadingPdf')}
        locale={locale}
        onFeedback={handleDocumentPreviewFeedback}
      />
    )
    : isDocxFile
      ? (
        <DocxPreview
          bytesBase64={bytesBase64}
          documentEpoch={documentEpoch}
          documentId={documentId}
          enabled={documentAssetsEnabled}
          loadingLabel={t('loadingDocx')}
          locale={locale}
          onFeedback={handleDocumentPreviewFeedback}
        />
      )
      : null;

  const {
    editorPaneRef,
    movePaneResize,
    previewPaneRef,
    resizePaneWithKeyboard,
    startPaneResize,
    stopPaneResize,
  } = usePaneResize({ editorPaneRatio, setEditorPaneRatio });
  const {
    moveWorkspaceSidebarResize,
    resizeWorkspaceSidebarWithKeyboard,
    startWorkspaceSidebarResize,
    stopWorkspaceSidebarResize,
  } = useWorkspaceSidebarResize({
    setSidebarWidth: setWorkspaceSidebarWidth,
    sidebarWidth: workspaceSidebarWidth,
  });
  const { closePopoutWindows, editorPopoutButton, openPanePopout, previewPopoutButton } = usePanePopouts({ broadcastPaneState, isPopout, setError, setNotice });
  const { forceCloseProgram } = useProgramCloseGuard({
    closePopoutWindows,
    dirty,
    flushWorkspaceSession,
    isPopout,
    setError,
    setNotice,
    setShowUnsavedExitPrompt,
  });

  useEffect(() => {
    const handleFeedbackError = (event: Event) => {
      if (event instanceof CustomEvent && typeof event.detail === 'string') {
        setError(event.detail);
        setNotice(null);
      }
    };
    window.addEventListener(APP_FEEDBACK_ERROR_EVENT, handleFeedbackError);
    return () => window.removeEventListener(APP_FEEDBACK_ERROR_EVENT, handleFeedbackError);
  }, [setError, setNotice]);

  useEffect(() => {
    if (isPopout) return undefined;
    let current = true;
    const update = nativeSaveMenuSyncRef.current
      .catch(() => undefined)
      .then(() => setNativeSaveMenuEnabled(nativeSaveMenuEnabled));
    nativeSaveMenuSyncRef.current = update;
    void update.catch((err: unknown) => {
      if (!current) return;
      setError(normalizeAppError(err, locale));
      setNotice(null);
    });
    return () => {
      current = false;
    };
  }, [isPopout, locale, nativeSaveMenuEnabled, setError, setNotice]);

  useEffect(() => {
    if (isPopout) return undefined;
    let disposed = false;
    let unlistenFeedback: (() => void) | undefined;
    listen<string>(APP_FEEDBACK_ERROR_EVENT, (event) => {
      setError(normalizeAppError(event.payload, locale));
      setNotice(null);
    }).then((fn) => {
      if (disposed) fn();
      else unlistenFeedback = fn;
    }).catch((err: unknown) => setError(normalizeAppError(err, locale)));
    return () => {
      disposed = true;
      unlistenFeedback?.();
    };
  }, [isPopout, locale, setError, setNotice]);

  useEffect(() => {
    if (!isPopout) return undefined;
    let disposed = false;
    let unlistenOutlineJump: (() => void) | undefined;
    listen<unknown>(OUTLINE_JUMP_EVENT, (event) => {
      const jump = decodeMarkdownOutlineJump(event.payload);
      if (
        !jump
        || jump.documentId !== documentId
        || jump.documentEpoch !== documentEpoch
      ) return;
      setOutlineJump(jump);
    }).then((fn) => {
      if (disposed) fn();
      else unlistenOutlineJump = fn;
    }).catch((err: unknown) => setError(normalizeAppError(err, locale)));
    return () => {
      disposed = true;
      unlistenOutlineJump?.();
    };
  }, [documentEpoch, documentId, isPopout, locale, setError]);

  useEffect(() => {
    if (isPopout) return undefined;
    let disposed = false;
    let unlistenNativeMenu: (() => void) | undefined;
    listen<unknown>(NATIVE_MENU_EVENT, (event) => {
      const command = decodeNativeMenuCommand(event.payload);
      if (!command) return;
      if (typeof command === 'object') {
        if (command.type === 'open-recent') void handleOpenRecent(command.entryId);
        else void handleClearRecent();
        return;
      }
      if (!nativeSaveMenuEnabled && (command === 'save' || command === 'save-as')) return;
      if (command === 'new') handleNew();
      else if (command === 'open-file') void handleOpenFile();
      else if (command === 'open-directory') void handleOpenDirectory();
      else if (command === 'save') void handleSave();
      else if (command === 'save-as') void handleSaveAs();
    }).then((fn) => {
      if (disposed) fn();
      else unlistenNativeMenu = fn;
    }).catch((err: unknown) => setError(normalizeAppError(err, locale)));
    return () => {
      disposed = true;
      unlistenNativeMenu?.();
    };
  }, [handleClearRecent, handleNew, handleOpenDirectory, handleOpenFile, handleOpenRecent, handleSave, handleSaveAs, isPopout, locale, nativeSaveMenuEnabled, setError]);

  const handleSaveAndQuit = useCallback(async () => {
    const saved = await saveCurrentDocument();
    if (!saved) return;
    setShowUnsavedExitPrompt(false);
    void forceCloseProgram().catch((err: unknown) => setError(normalizeAppError(err, locale)));
  }, [forceCloseProgram, locale, saveCurrentDocument, setError]);

  const handleCancelExit = useCallback(() => {
    setShowUnsavedExitPrompt(false);
  }, []);

  const handleQuitWithoutSaving = useCallback(() => {
    setShowUnsavedExitPrompt(false);
    void forceCloseProgram().catch((err: unknown) => setError(normalizeAppError(err, locale)));
  }, [forceCloseProgram, locale, setError]);

  const requestWorkspaceFileOpen = useCallback((path: string) => {
    if (path === activePath) return;
    if (dirty) {
      setError(null);
      setNotice(null);
      setPendingFileSwitchPath(path);
      return;
    }
    void openWorkspaceFilePath(path);
  }, [activePath, dirty, openWorkspaceFilePath, setError, setNotice]);

  const handleCancelFileSwitch = useCallback(() => {
    setPendingFileSwitchPath(null);
  }, []);

  const handleFileSwitchWithoutSaving = useCallback(() => {
    const targetPath = pendingFileSwitchPath;
    if (!targetPath) return;
    setPendingFileSwitchPath(null);
    void openWorkspaceFilePath(targetPath);
  }, [openWorkspaceFilePath, pendingFileSwitchPath]);

  const handleSaveAndSwitchFile = useCallback(async () => {
    const targetPath = pendingFileSwitchPath;
    if (!targetPath) return;
    const saved = await saveCurrentDocument();
    if (!saved) return;
    setPendingFileSwitchPath(null);
    await openWorkspaceFilePath(targetPath);
  }, [openWorkspaceFilePath, pendingFileSwitchPath, saveCurrentDocument]);

  const dismissFeedbackDialog = useCallback(() => {
    setError(null);
    setNotice(null);
  }, [setError, setNotice]);

  const handleWorkspaceEntryConfirm = useCallback((name?: string) => {
    const operation = workspaceEntryOperation;
    if (!operation) return;
    setWorkspaceEntryOperation(null);
    if (operation.kind === 'create-file') {
      void createFileInWorkspace(operation.parentPath, name ?? '', operation.fileKind);
    } else if (operation.kind === 'create-folder') {
      void createFolderInWorkspace(operation.parentPath, name ?? '');
    } else if (operation.kind === 'rename') {
      void renameWorkspaceEntryPath(operation.path, name ?? '');
    } else {
      void deleteWorkspaceEntryPath(operation.path);
    }
  }, [createFileInWorkspace, createFolderInWorkspace, deleteWorkspaceEntryPath, renameWorkspaceEntryPath, workspaceEntryOperation]);

  const handleWorkspaceMoveConfirm = useCallback((destinationParentPath: string) => {
    const operation = workspaceMoveOperation;
    if (!operation) return;
    setWorkspaceMoveOperation(null);
    void moveWorkspaceEntryPath(operation.path, destinationParentPath);
  }, [moveWorkspaceEntryPath, workspaceMoveOperation]);

  const toggleFolder = useCallback((path: string) => {
    setCollapsedFolders((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const handleOutlineItemSelect = useCallback((item: MarkdownOutlineItem) => {
    outlineJumpRequestIdRef.current += 1;
    const jump: MarkdownOutlineJump = {
      documentId,
      documentEpoch,
      item,
      requestId: outlineJumpRequestIdRef.current,
    };
    setOutlineJump(jump);
    void emit(OUTLINE_JUMP_EVENT, jump).catch((err: unknown) => {
      setError(normalizeAppError(err, locale));
      setNotice(null);
    });
  }, [documentEpoch, documentId, locale, setError, setNotice]);

  const handleWorkspaceAssetInsert = useCallback((
    asset: WorkspaceFileEntry,
    position: { clientX: number; clientY: number },
  ) => {
    if (
      activeFileKind !== 'markdown'
      || authorityStatus !== 'committed'
      || !activeWorkspaceMarkdownFile
    ) return;
    const markdown = createMarkdownMediaReference(asset, activeWorkspaceMarkdownFile);
    if (!markdown) return;
    mediaInsertionRequestIdRef.current += 1;
    setMediaInsertion({
      ...position,
      documentEpoch,
      documentId,
      markdown,
      requestId: mediaInsertionRequestIdRef.current,
    });
  }, [activeFileKind, activeWorkspaceMarkdownFile, authorityStatus, documentEpoch, documentId]);

  if (popoutPane === 'editor') {
    return (
      <PopoutPaneShell>
        {feedbackDialog && <FeedbackDialog dialog={feedbackDialog} onDismiss={dismissFeedbackDialog} />}
        {isExcalidrawFile
          ? (
            <ExcalidrawPane
              activePath={activePath}
              content={content}
              documentEpoch={documentEpoch}
              documentId={documentId}
              editable={authorityStatus === 'committed'}
              loadingLabel={t('loadingExcalidraw')}
              locale={locale}
              onContentChange={updateContent}
              onInvalidScene={handleExcalidrawError}
              popout
            />
          )
          : isDocumentFile
          ? <PreviewPane dirty={dirty} popout>{documentPreview}</PreviewPane>
          : isImageFile && activePath
          ? <WorkspaceImagePreview key={activePath} enabled={documentAssetsEnabled} path={activePath} popout previewRevision={previewRevision} />
          : isMediaFile && activePath
            ? <WorkspaceMediaPreview key={activePath} enabled={documentAssetsEnabled} kind={mediaKind} mimeType={mediaMimeType} path={activePath} popout previewRevision={previewRevision} />
            : <EditorPane activePath={activePath} content={content} documentEpoch={documentEpoch} documentId={documentId} editable={authorityStatus === 'committed'} fileKind={editorFileKind} outlineJump={currentOutlineJump} onContentChange={updateContent} popout />}
      </PopoutPaneShell>
    );
  }

  if (popoutPane === 'preview') {
    return (
      <PopoutPaneShell>
        {feedbackDialog && <FeedbackDialog dialog={feedbackDialog} onDismiss={dismissFeedbackDialog} />}
        {isExcalidrawFile
          ? (
            <ExcalidrawPane
              activePath={activePath}
              content={content}
              documentEpoch={documentEpoch}
              documentId={documentId}
              editable={false}
              loadingLabel={t('loadingExcalidraw')}
              locale={locale}
              onContentChange={updateContent}
              onInvalidScene={handleExcalidrawError}
              popout
            />
          )
          : isDocumentFile
          ? <PreviewPane dirty={dirty} popout>{documentPreview}</PreviewPane>
          : isImageFile && activePath
          ? <WorkspaceImagePreview key={activePath} enabled={documentAssetsEnabled} path={activePath} popout previewRevision={previewRevision} />
          : isMediaFile && activePath
            ? <WorkspaceMediaPreview key={activePath} enabled={documentAssetsEnabled} kind={mediaKind} mimeType={mediaMimeType} path={activePath} popout previewRevision={previewRevision} />
          : (
            <PreviewPane dirty={dirty} outlineJump={currentOutlineJump} popout>
              {activePresentation.preview === 'html' && activePath
                ? <WorkspaceHtmlPreview content={content} enabled={documentAssetsEnabled} path={activePath} />
                : <JinxiuMarkdown currentFilePath={activePath} localAssetsEnabled={documentAssetsEnabled} workspaceRoot={workspaceRoot}>{content}</JinxiuMarkdown>}
            </PreviewPane>
          )}
      </PopoutPaneShell>
    );
  }

  return (
    <div className="app-shell">
      <AppToolbar
        activePath={activePath}
        busy={busy}
        dirty={dirty}
      />

      {externalFileAction ? (
        <ExternalFileChangeDialog
          action={externalFileAction}
          onCloseDeletedDraft={() => void handleCloseDeletedDraft()}
          onKeepCurrent={() => void handleKeepCurrentExternal()}
          onSaveDeletedDraftAs={() => void handleSaveDeletedDraftAs()}
          onUseExternal={() => void handleUseExternal()}
        />
      ) : showUnsavedExitPrompt ? (
        <UnsavedExitDialog
          busy={busy}
          prompt={unsavedExitPrompt}
          onCancelExit={handleCancelExit}
          onQuitWithoutSaving={handleQuitWithoutSaving}
          onSaveAndQuit={handleSaveAndQuit}
        />
      ) : unsavedFileSwitchPrompt ? (
        <UnsavedExitDialog
          busy={busy}
          prompt={unsavedFileSwitchPrompt}
          onCancelExit={handleCancelFileSwitch}
          onQuitWithoutSaving={handleFileSwitchWithoutSaving}
          onSaveAndQuit={() => void handleSaveAndSwitchFile()}
        />
      ) : workspaceEntryOperation ? (
        <WorkspaceEntryDialog
          busy={busy}
          operation={workspaceEntryOperation}
          onCancel={() => setWorkspaceEntryOperation(null)}
          onConfirm={handleWorkspaceEntryConfirm}
        />
      ) : workspaceMoveOperation ? (
        <WorkspaceMoveDialog
          busy={busy}
          destinations={workspaceMoveDestinations}
          operation={workspaceMoveOperation}
          onCancel={() => setWorkspaceMoveOperation(null)}
          onConfirm={handleWorkspaceMoveConfirm}
        />
      ) : feedbackDialog ? (
        <FeedbackDialog dialog={feedbackDialog} onDismiss={dismissFeedbackDialog} />
      ) : null}

      <main className={getWorkspaceLayoutClassName(fileTreeCollapsed, activeFileKind)} style={workspaceLayoutStyle}>
        <FileSidebar
          activePath={activePath}
          collapsed={fileTreeCollapsed}
          collapsedFolders={collapsedFolders}
          disabled={busy || externalFileAction !== null || pendingFileSwitchPath !== null}
          fileTree={fileTree}
          onCollapseChange={setFileTreeCollapsed}
          onCreateFile={(parentPath, parentName, fileKind) => setWorkspaceEntryOperation({
            fileKind,
            kind: 'create-file',
            parentName,
            parentPath,
          })}
          onCreateFolder={(parentPath, parentName) => setWorkspaceEntryOperation({ kind: 'create-folder', parentName, parentPath })}
          onDeleteEntry={(path, currentName, entryKind) => setWorkspaceEntryOperation({ currentName, entryKind, kind: 'delete', path })}
          onInsertWorkspaceAsset={activeFileKind === 'markdown'
            && authorityStatus === 'committed'
            && activeWorkspaceMarkdownFile
            ? handleWorkspaceAssetInsert
            : undefined}
          onMoveEntry={(path, destinationParentPath) => void moveWorkspaceEntryPath(path, destinationParentPath)}
          onOpenFile={requestWorkspaceFileOpen}
          onRenameEntry={(path, newName) => void renameWorkspaceEntryPath(path, newName)}
          onRequestMove={(target) => setWorkspaceMoveOperation({
            currentName: target.name,
            entryKind: target.kind,
            path: target.path,
          })}
          onSelectOutlineItem={handleOutlineItemSelect}
          onRefreshWorkspace={() => void refreshWorkspace()}
          onToggleFolder={toggleFolder}
          outlineItems={outlineItems}
          workspaceRoot={workspaceRoot}
        />

        {!fileTreeCollapsed && (
          <WorkspaceSidebarResizer
            sidebarWidth={workspaceSidebarWidth}
            onKeyDown={resizeWorkspaceSidebarWithKeyboard}
            onPointerCancel={stopWorkspaceSidebarResize}
            onPointerDown={startWorkspaceSidebarResize}
            onPointerMove={moveWorkspaceSidebarResize}
            onPointerUp={stopWorkspaceSidebarResize}
          />
        )}

        {isExcalidrawFile ? (
          <ExcalidrawPane
            activePath={activePath}
            content={content}
            documentEpoch={documentEpoch}
            documentId={documentId}
            editable={authorityStatus === 'committed'}
            loadingLabel={t('loadingExcalidraw')}
            locale={locale}
            paneRef={editorPaneRef}
            popoutButton={editorPopoutButton}
            onContentChange={updateContent}
            onInvalidScene={handleExcalidrawError}
            onPopout={() => void openPanePopout('editor')}
          />
        ) : isDocumentFile ? (
          <PreviewPane
            dirty={dirty}
            paneRef={previewPaneRef}
            popoutButton={previewPopoutButton}
            onPopout={() => void openPanePopout('preview')}
          >
            {documentPreview}
          </PreviewPane>
        ) : isImageFile && activePath ? (
          <WorkspaceImagePreview
            key={activePath}
            enabled={documentAssetsEnabled}
            path={activePath}
            paneRef={previewPaneRef}
            popoutButton={previewPopoutButton}
            previewRevision={previewRevision}
            onPopout={() => void openPanePopout('preview')}
          />
        ) : isMediaFile && activePath ? (
          <WorkspaceMediaPreview
            key={activePath}
            enabled={documentAssetsEnabled}
            kind={mediaKind}
            mimeType={mediaMimeType}
            path={activePath}
            paneRef={previewPaneRef}
            popoutButton={previewPopoutButton}
            previewRevision={previewRevision}
            onPopout={() => void openPanePopout('preview')}
          />
        ) : (
          <>
            <EditorPane
              activePath={activePath}
              content={content}
              documentEpoch={documentEpoch}
              documentId={documentId}
              editable={authorityStatus === 'committed'}
              fileKind={editorFileKind}
              mediaInsertion={currentMediaInsertion}
              outlineJump={currentOutlineJump}
              paneRef={editorPaneRef}
              popoutButton={editorPopoutButton}
              onContentChange={updateContent}
              onPopout={() => void openPanePopout('editor')}
            />

            <PaneResizer
              editorPaneRatio={editorPaneRatio}
              onKeyDown={resizePaneWithKeyboard}
              onPointerCancel={stopPaneResize}
              onPointerDown={startPaneResize}
              onPointerMove={movePaneResize}
              onPointerUp={stopPaneResize}
            />

            <PreviewPane
              dirty={dirty}
              outlineJump={currentOutlineJump}
              paneRef={previewPaneRef}
              popoutButton={previewPopoutButton}
              onPopout={() => void openPanePopout('preview')}
            >
              {activePresentation.preview === 'html' && activePath
                ? <WorkspaceHtmlPreview content={content} enabled={documentAssetsEnabled} path={activePath} />
                : <JinxiuMarkdown currentFilePath={activePath} localAssetsEnabled={documentAssetsEnabled} workspaceRoot={workspaceRoot}>{content}</JinxiuMarkdown>}
            </PreviewPane>
          </>
        )}
      </main>
    </div>
  );
}
