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
import { emit, emitTo, listen } from '@tauri-apps/api/event';
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
import {
  getPaneLayoutStyle,
  getPanePopoutLabel,
  parsePopoutInstanceId,
  parsePopoutPane,
} from './lib/paneLayout';
import { loadLazyModuleWithRetry } from './lib/lazyModule';
import {
  decodeMarkdownOutlineJump,
  extractMarkdownOutline,
  OUTLINE_JUMP_EVENT,
  type MarkdownOutlineItem,
  type MarkdownOutlineJump,
} from './lib/markdownOutline';
import {
  createMarkdownMediaReference,
  decodeMarkdownMediaCursorInsertion,
  decodeMarkdownMediaInsertionHandshake,
  decodeMarkdownMediaInsertionReady,
  decodeMarkdownMediaInsertionReadyRequest,
  MARKDOWN_MEDIA_INSERTION_EVENT,
  MARKDOWN_MEDIA_INSERTION_HANDSHAKE_ACK_EVENT,
  MARKDOWN_MEDIA_INSERTION_HANDSHAKE_EVENT,
  MARKDOWN_MEDIA_INSERTION_REQUEST_READY_EVENT,
  MARKDOWN_MEDIA_INSERTION_READY_EVENT,
  type MarkdownMediaCursorInsertion,
  type MarkdownMediaInsertionHandshake,
  type MarkdownMediaInsertionReady,
  type MarkdownMediaInsertion,
  type MarkdownMediaInsertionTarget,
} from './lib/markdownMedia';
import { createPaneProtocolId } from './lib/tauriPaneReplication';
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
const LazyExcalidrawPane = lazy(async () => {
  try {
    const module = await loadLazyModuleWithRetry(() => import('./components/ExcalidrawPane'));
    return { default: module.ExcalidrawPane };
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    throw new Error(`Failed to load Excalidraw preview module: ${detail}`);
  }
});
const LazyPdfPreview = lazy(() => import('./components/PdfPreview').then((module) => ({
  default: module.PdfPreview,
})));
const MAIN_WINDOW_LABEL = 'main';

interface PendingMarkdownMediaInsertionHandshake {
  attempt: number;
  generation: number;
  handshake: MarkdownMediaInsertionHandshake;
  retryTimer: ReturnType<typeof globalThis.setTimeout> | null;
  sending: boolean;
}

type PendingMarkdownMediaCursorInsertion = Omit<MarkdownMediaCursorInsertion, 'popoutInstanceId'>;

interface MarkdownMediaRetryController {
  cancelled: boolean;
  pendingTimers: Map<ReturnType<typeof globalThis.setTimeout>, () => void>;
}

const MEDIA_EVENT_RETRY_DELAYS_MS = [0, 100, 250] as const;
const MEDIA_INSERTION_HANDSHAKE_ATTEMPTS = 3;
const MEDIA_INSERTION_HANDSHAKE_ACK_TIMEOUT_MS = 250;

function createMarkdownMediaRetryController(): MarkdownMediaRetryController {
  return { cancelled: false, pendingTimers: new Map() };
}

function cancelMarkdownMediaRetries(controller: MarkdownMediaRetryController): void {
  controller.cancelled = true;
  for (const [timer, resolve] of controller.pendingTimers) {
    globalThis.clearTimeout(timer);
    resolve();
  }
  controller.pendingTimers.clear();
}

function waitForRetry(
  delayMs: number,
  controller: MarkdownMediaRetryController | undefined,
): Promise<boolean> {
  if (controller?.cancelled) return Promise.resolve(false);
  return new Promise((resolve) => {
    const complete = () => resolve(!controller?.cancelled);
    const timer = globalThis.setTimeout(() => {
      controller?.pendingTimers.delete(timer);
      complete();
    }, delayMs);
    controller?.pendingTimers.set(timer, complete);
  });
}

function cancelMarkdownMediaInsertionHandshake(
  pending: PendingMarkdownMediaInsertionHandshake | null,
): void {
  if (pending?.retryTimer !== null && pending?.retryTimer !== undefined) {
    globalThis.clearTimeout(pending.retryTimer);
    pending.retryTimer = null;
  }
}

async function emitToWithRetry(
  target: string,
  event: string,
  payload: unknown,
  isCurrent: () => boolean,
  retryController?: MarkdownMediaRetryController,
): Promise<void> {
  let lastError: unknown;
  for (const delayMs of MEDIA_EVENT_RETRY_DELAYS_MS) {
    if (delayMs > 0 && !await waitForRetry(delayMs, retryController)) return;
    if (retryController?.cancelled || !isCurrent()) return;
    try {
      await emitTo(target, event, payload);
      return;
    } catch (err) {
      lastError = err;
    }
  }
  if (!retryController?.cancelled && isCurrent()) throw lastError;
}

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

function currentEditorPopoutInstanceId() {
  return typeof window === 'undefined' ? null : parsePopoutInstanceId(window.location.search);
}

function getWorkspaceRelativePath(workspaceRoot: string | null, path: string | null): string | null {
  if (!workspaceRoot || !path) return null;
  const normalizedRoot = workspaceRoot.replace(/\\/g, '/').replace(/\/+$/, '') || '/';
  const normalizedPath = path.replace(/\\/g, '/');
  const prefix = normalizedRoot === '/' ? '/' : `${normalizedRoot}/`;
  if (!normalizedPath.startsWith(prefix)) return null;
  const relativePath = normalizedPath.slice(prefix.length);
  if (!relativePath || relativePath.split('/').some((segment) => !segment || segment === '.' || segment === '..')) return null;
  return relativePath;
}

export default function App() {
  const { locale, t } = useI18n();
  const popoutPane = useMemo(() => currentPopoutPane(), []);
  const isPopout = popoutPane !== 'main';
  const [editorPopoutInstanceId] = useState(() => (
    popoutPane === 'editor'
      ? currentEditorPopoutInstanceId() ?? createPaneProtocolId('markdown-media-popout')
      : null
  ));
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
  const mediaInsertionHighWaterRef = useRef(new Map<string, number>());
  const pendingPopoutMediaInsertionsRef = useRef<PendingMarkdownMediaCursorInsertion[]>([]);
  const popoutMediaInsertionTailRef = useRef<Promise<void>>(Promise.resolve());
  const popoutMediaInsertionGenerationRef = useRef(0);
  const editorPopoutReadyRef = useRef<MarkdownMediaInsertionReady | null>(null);
  const editorPopoutHandshakeRef = useRef<PendingMarkdownMediaInsertionHandshake | null>(null);
  const startEditorPopoutHandshakeRef = useRef<((ready: MarkdownMediaInsertionReady) => void) | null>(null);
  const editorPopoutOpenRef = useRef(false);
  const expectedEditorPopoutInstanceIdRef = useRef<string | null>(null);
  const pendingEditorPopoutReadyRequestIdRef = useRef<string | null>(null);
  const editorPopoutOpenRequestRef = useRef<Promise<void> | null>(null);
  const markdownMediaRetryControllerRef = useRef(createMarkdownMediaRetryController());
  const mountedRef = useRef(true);
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
  const editorPopoutOpen = editorPopoutButton?.isPoppedOut === true;
  editorPopoutOpenRef.current = editorPopoutOpen || editorPopoutOpenRequestRef.current !== null;
  const { forceCloseProgram } = useProgramCloseGuard({
    closePopoutWindows,
    dirty,
    flushWorkspaceSession,
    isPopout,
    setError,
    setNotice,
    setShowUnsavedExitPrompt,
  });

  const sendCursorInsertionToEditorPopout = useCallback((insertion: MarkdownMediaCursorInsertion) => {
    const generation = popoutMediaInsertionGenerationRef.current;
    const delivery = popoutMediaInsertionTailRef.current
      .catch(() => undefined)
      .then(() => {
        return emitToWithRetry(
          getPanePopoutLabel('editor'),
          MARKDOWN_MEDIA_INSERTION_EVENT,
          insertion,
          () => generation === popoutMediaInsertionGenerationRef.current
            && editorPopoutOpenRef.current
            && mountedRef.current,
          markdownMediaRetryControllerRef.current,
        );
      });
    popoutMediaInsertionTailRef.current = delivery;
    void delivery.catch((err: unknown) => {
      if (generation !== popoutMediaInsertionGenerationRef.current || !mountedRef.current) return;
      setError(normalizeAppError(err, locale));
      setNotice(null);
    });
  }, [locale, setError, setNotice]);

  useEffect(() => {
    const retryController = createMarkdownMediaRetryController();
    markdownMediaRetryControllerRef.current = retryController;
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      cancelMarkdownMediaRetries(retryController);
    };
  }, []);

  useEffect(() => {
    popoutMediaInsertionGenerationRef.current += 1;
    cancelMarkdownMediaInsertionHandshake(editorPopoutHandshakeRef.current);
    editorPopoutHandshakeRef.current = null;
    const ready = editorPopoutReadyRef.current;
    if (
      ready
      && (ready.documentId !== documentId || ready.documentEpoch !== documentEpoch)
    ) editorPopoutReadyRef.current = null;
    pendingEditorPopoutReadyRequestIdRef.current = null;
    pendingPopoutMediaInsertionsRef.current = pendingPopoutMediaInsertionsRef.current.filter((insertion) => (
      insertion.documentId === documentId && insertion.documentEpoch === documentEpoch
    ));
  }, [documentEpoch, documentId]);

  useEffect(() => () => {
    editorPopoutOpenRequestRef.current = null;
  }, []);

  useEffect(() => {
    if (editorPopoutOpen) return;
    popoutMediaInsertionGenerationRef.current += 1;
    cancelMarkdownMediaInsertionHandshake(editorPopoutHandshakeRef.current);
    editorPopoutHandshakeRef.current = null;
    editorPopoutReadyRef.current = null;
    expectedEditorPopoutInstanceIdRef.current = null;
    pendingEditorPopoutReadyRequestIdRef.current = null;
    pendingPopoutMediaInsertionsRef.current = [];
  }, [editorPopoutOpen]);

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
    const unlisteners: Array<() => void> = [];
    const isHandshakeCurrent = (pending: PendingMarkdownMediaInsertionHandshake) => (
      !disposed
      && editorPopoutOpenRef.current
      && pending.generation === popoutMediaInsertionGenerationRef.current
      && editorPopoutHandshakeRef.current === pending
      && pending.handshake.documentId === documentId
      && pending.handshake.documentEpoch === documentEpoch
      && pending.handshake.popoutInstanceId === expectedEditorPopoutInstanceIdRef.current
    );
    const failHandshake = (pending: PendingMarkdownMediaInsertionHandshake, _error: unknown) => {
      if (!isHandshakeCurrent(pending)) return;
      cancelMarkdownMediaInsertionHandshake(pending);
      editorPopoutHandshakeRef.current = null;
      setError(t('popoutInsertionUnavailable'));
      setNotice(null);
    };
    function scheduleHandshakeRetry(
      pending: PendingMarkdownMediaInsertionHandshake,
      delayMs: number,
      failure: unknown,
    ) {
      cancelMarkdownMediaInsertionHandshake(pending);
      pending.retryTimer = globalThis.setTimeout(() => {
        pending.retryTimer = null;
        if (!isHandshakeCurrent(pending)) return;
        if (pending.attempt >= MEDIA_INSERTION_HANDSHAKE_ATTEMPTS) {
          failHandshake(pending, failure);
          return;
        }
        pending.attempt += 1;
        sendHandshake(pending);
      }, delayMs);
    }
    function sendHandshake(pending: PendingMarkdownMediaInsertionHandshake) {
      if (!isHandshakeCurrent(pending) || pending.sending) return;
      pending.sending = true;
      void emitTo(
        getPanePopoutLabel('editor'),
        MARKDOWN_MEDIA_INSERTION_HANDSHAKE_EVENT,
        pending.handshake,
      ).then(() => {
        pending.sending = false;
        if (!isHandshakeCurrent(pending)) return;
        scheduleHandshakeRetry(
          pending,
          MEDIA_INSERTION_HANDSHAKE_ACK_TIMEOUT_MS,
          new Error('Markdown media insertion handshake timed out'),
        );
      }).catch((err: unknown) => {
        pending.sending = false;
        if (!isHandshakeCurrent(pending)) return;
        scheduleHandshakeRetry(
          pending,
          MEDIA_EVENT_RETRY_DELAYS_MS[pending.attempt] ?? MEDIA_INSERTION_HANDSHAKE_ACK_TIMEOUT_MS,
          err,
        );
      });
    }
    const startHandshake = (ready: MarkdownMediaInsertionReady) => {
      if (disposed || !editorPopoutOpenRef.current) return;
      const current = editorPopoutHandshakeRef.current;
      if (
        current
        && isHandshakeCurrent(current)
        && current.handshake.documentId === ready.documentId
        && current.handshake.documentEpoch === ready.documentEpoch
        && current.handshake.popoutInstanceId === ready.popoutInstanceId
      ) {
        if (current.sending || current.attempt >= MEDIA_INSERTION_HANDSHAKE_ATTEMPTS) return;
        cancelMarkdownMediaInsertionHandshake(current);
        current.attempt += 1;
        sendHandshake(current);
        return;
      }
      cancelMarkdownMediaInsertionHandshake(editorPopoutHandshakeRef.current);
      editorPopoutReadyRef.current = null;
      const { readyRequestId: _readyRequestId, ...handshakeReady } = ready;
      const pending: PendingMarkdownMediaInsertionHandshake = {
        attempt: 1,
        generation: popoutMediaInsertionGenerationRef.current,
        handshake: {
          ...handshakeReady,
          handshakeId: createPaneProtocolId('markdown-media-handshake'),
        },
        retryTimer: null,
        sending: false,
      };
      editorPopoutHandshakeRef.current = pending;
      sendHandshake(pending);
    };
    startEditorPopoutHandshakeRef.current = startHandshake;
    const handleMediaInsertionReady = (event: { payload: unknown }) => {
      if (disposed) return;
      const ready = decodeMarkdownMediaInsertionReady(event.payload);
      if (
        !ready
        || ready.documentId !== documentId
        || ready.documentEpoch !== documentEpoch
        || !editorPopoutOpenRef.current
      ) return;
      const expectedInstanceId = expectedEditorPopoutInstanceIdRef.current;
      const pendingReadyRequestId = pendingEditorPopoutReadyRequestIdRef.current;
      if (expectedInstanceId && ready.popoutInstanceId !== expectedInstanceId) return;
      if (pendingReadyRequestId && ready.readyRequestId !== pendingReadyRequestId) return;
      if (!expectedInstanceId) expectedEditorPopoutInstanceIdRef.current = ready.popoutInstanceId;
      if (pendingReadyRequestId) pendingEditorPopoutReadyRequestIdRef.current = null;
      startHandshake(ready);
    };
    const handleMediaInsertionHandshakeAck = (event: { payload: unknown }) => {
      if (disposed) return;
      const handshake = decodeMarkdownMediaInsertionHandshake(event.payload);
      const pendingHandshake = editorPopoutHandshakeRef.current;
      if (
        !handshake
        || !pendingHandshake
        || !editorPopoutOpenRef.current
        || pendingHandshake.generation !== popoutMediaInsertionGenerationRef.current
        || handshake.handshakeId !== pendingHandshake.handshake.handshakeId
        || handshake.documentId !== pendingHandshake.handshake.documentId
        || handshake.documentEpoch !== pendingHandshake.handshake.documentEpoch
        || handshake.popoutInstanceId !== pendingHandshake.handshake.popoutInstanceId
        || handshake.popoutInstanceId !== expectedEditorPopoutInstanceIdRef.current
        || handshake.documentId !== documentId
        || handshake.documentEpoch !== documentEpoch
      ) return;
      cancelMarkdownMediaInsertionHandshake(pendingHandshake);
      editorPopoutHandshakeRef.current = null;
      editorPopoutReadyRef.current = {
        documentId: handshake.documentId,
        documentEpoch: handshake.documentEpoch,
        popoutInstanceId: handshake.popoutInstanceId,
      };
      const pending = pendingPopoutMediaInsertionsRef.current.filter((insertion) => (
        insertion.documentId === handshake.documentId && insertion.documentEpoch === handshake.documentEpoch
      ));
      pendingPopoutMediaInsertionsRef.current = pendingPopoutMediaInsertionsRef.current.filter((insertion) => (
        insertion.documentId !== handshake.documentId || insertion.documentEpoch !== handshake.documentEpoch
      ));
      for (const insertion of pending) {
        sendCursorInsertionToEditorPopout({
          ...insertion,
          popoutInstanceId: handshake.popoutInstanceId,
        });
      }
    };
    void Promise.allSettled([
      listen<unknown>(MARKDOWN_MEDIA_INSERTION_READY_EVENT, handleMediaInsertionReady),
      listen<unknown>(MARKDOWN_MEDIA_INSERTION_HANDSHAKE_ACK_EVENT, handleMediaInsertionHandshakeAck),
    ]).then((registrations) => {
      const registered = registrations.flatMap((registration) => (
        registration.status === 'fulfilled' ? [registration.value] : []
      ));
      const failure = registrations.find((registration) => registration.status === 'rejected');
      if (disposed || failure) {
        for (const unlisten of registered) unlisten();
      } else {
        unlisteners.push(...registered);
        if (
          editorPopoutOpenRef.current
          && pendingPopoutMediaInsertionsRef.current.some((insertion) => (
            insertion.documentId === documentId && insertion.documentEpoch === documentEpoch
          ))
        ) {
          const expectedInstanceId = expectedEditorPopoutInstanceIdRef.current;
          if (expectedInstanceId) {
            startHandshake({
              documentEpoch,
              documentId,
              popoutInstanceId: expectedInstanceId,
            });
          }
        }
      }
      if (!disposed && failure?.status === 'rejected') {
        setError(normalizeAppError(failure.reason, locale));
        setNotice(null);
      }
    });
    return () => {
      disposed = true;
      if (startEditorPopoutHandshakeRef.current === startHandshake) {
        startEditorPopoutHandshakeRef.current = null;
      }
      const pendingHandshake = editorPopoutHandshakeRef.current;
      if (
        pendingHandshake
        && pendingHandshake.handshake.documentId === documentId
        && pendingHandshake.handshake.documentEpoch === documentEpoch
      ) {
        cancelMarkdownMediaInsertionHandshake(pendingHandshake);
        editorPopoutHandshakeRef.current = null;
      }
      for (const unlisten of unlisteners) unlisten();
    };
  }, [documentEpoch, documentId, isPopout, locale, sendCursorInsertionToEditorPopout, setError, setNotice, t]);

  useEffect(() => {
    if (
      popoutPane !== 'editor'
      || activeFileKind !== 'markdown'
      || authorityStatus !== 'committed'
      || !editorPopoutInstanceId
    ) return undefined;
    const popoutInstanceId = editorPopoutInstanceId;
    let disposed = false;
    let handshakeReceived = false;
    const unlisteners: Array<() => void> = [];
    const announceReady = (readyRequestId?: string) => emitToWithRetry(
      MAIN_WINDOW_LABEL,
      MARKDOWN_MEDIA_INSERTION_READY_EVENT,
      readyRequestId
        ? { documentEpoch, documentId, popoutInstanceId, readyRequestId }
        : { documentEpoch, documentId, popoutInstanceId },
      () => !disposed,
      markdownMediaRetryControllerRef.current,
    ).catch((err: unknown) => {
      if (disposed) return;
      setError(normalizeAppError(err, locale));
      setNotice(null);
    });
    const handleMediaInsertion = (event: { payload: unknown }) => {
      if (disposed) return;
      const insertion = decodeMarkdownMediaCursorInsertion(event.payload);
      if (
        !insertion
        || insertion.documentId !== documentId
        || insertion.documentEpoch !== documentEpoch
        || insertion.popoutInstanceId !== popoutInstanceId
        || activeFileKind !== 'markdown'
        || authorityStatus !== 'committed'
      ) return;
      const requestKey = `${insertion.documentId}:${insertion.documentEpoch}`;
      const highestRequestId = mediaInsertionHighWaterRef.current.get(requestKey) ?? 0;
      const currentRelativePath = getWorkspaceRelativePath(workspaceRoot, activePath);
      if (insertion.requestId <= highestRequestId || currentRelativePath !== insertion.documentRelativePath) return;
      const markdown = createMarkdownMediaReference(insertion.asset, { relative_path: currentRelativePath });
      if (!markdown) return;
      mediaInsertionHighWaterRef.current.set(requestKey, insertion.requestId);
      setMediaInsertion({
        documentEpoch: insertion.documentEpoch,
        documentId: insertion.documentId,
        markdown,
        requestId: insertion.requestId,
        target: { kind: 'cursor' },
      });
    };
    const handleMediaInsertionHandshake = (event: { payload: unknown }) => {
      if (disposed) return;
      const handshake = decodeMarkdownMediaInsertionHandshake(event.payload);
      if (
        !handshake
        || handshake.documentId !== documentId
        || handshake.documentEpoch !== documentEpoch
        || handshake.popoutInstanceId !== popoutInstanceId
        || activeFileKind !== 'markdown'
        || authorityStatus !== 'committed'
      ) return;
      handshakeReceived = true;
      void emitToWithRetry(
        MAIN_WINDOW_LABEL,
        MARKDOWN_MEDIA_INSERTION_HANDSHAKE_ACK_EVENT,
        handshake,
        () => !disposed,
        markdownMediaRetryControllerRef.current,
      ).catch((err: unknown) => {
        if (disposed) return;
        setError(normalizeAppError(err, locale));
        setNotice(null);
      });
    };
    const handleMediaInsertionReadyRequest = (event: { payload: unknown }) => {
      if (disposed) return;
      const request = decodeMarkdownMediaInsertionReadyRequest(event.payload);
      if (!request || request.documentId !== documentId || request.documentEpoch !== documentEpoch) return;
      void announceReady(request.readyRequestId);
    };
    void Promise.allSettled([
      listen<unknown>(MARKDOWN_MEDIA_INSERTION_EVENT, handleMediaInsertion),
      listen<unknown>(MARKDOWN_MEDIA_INSERTION_HANDSHAKE_EVENT, handleMediaInsertionHandshake),
      listen<unknown>(MARKDOWN_MEDIA_INSERTION_REQUEST_READY_EVENT, handleMediaInsertionReadyRequest),
    ]).then((registrations) => {
      const registered = registrations.flatMap((registration) => (
        registration.status === 'fulfilled' ? [registration.value] : []
      ));
      const failure = registrations.find((registration) => registration.status === 'rejected');
      if (disposed || failure) {
        for (const unlisten of registered) unlisten();
      } else {
        unlisteners.push(...registered);
        if (!handshakeReceived) void announceReady();
      }
      if (!disposed && failure?.status === 'rejected') {
        setError(normalizeAppError(failure.reason, locale));
        setNotice(null);
      }
    });
    return () => {
      disposed = true;
      for (const unlisten of unlisteners) unlisten();
    };
  }, [activeFileKind, activePath, authorityStatus, documentEpoch, documentId, editorPopoutInstanceId, locale, popoutPane, setError, setNotice, workspaceRoot]);

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

  const requestEditorPopoutReady = useCallback(() => {
    if (pendingEditorPopoutReadyRequestIdRef.current) return;
    const readyRequestId = createPaneProtocolId('markdown-media-ready-request');
    pendingEditorPopoutReadyRequestIdRef.current = readyRequestId;
    editorPopoutReadyRef.current = null;
    cancelMarkdownMediaInsertionHandshake(editorPopoutHandshakeRef.current);
    editorPopoutHandshakeRef.current = null;
    editorPopoutOpenRef.current = true;
    void emitToWithRetry(
      getPanePopoutLabel('editor'),
      MARKDOWN_MEDIA_INSERTION_REQUEST_READY_EVENT,
      { documentEpoch, documentId, readyRequestId },
      () => mountedRef.current,
      markdownMediaRetryControllerRef.current,
    ).catch((err: unknown) => {
      if (!mountedRef.current) return;
      setError(normalizeAppError(err, locale));
      setNotice(null);
    });
  }, [documentEpoch, documentId, locale, setError, setNotice]);

  const handleEditorPopoutOpen = useCallback(() => {
    if (editorPopoutOpen) {
      void openPanePopout('editor').then((outcome) => {
        if (outcome.status !== 'existing') return;
        const ready = editorPopoutReadyRef.current;
        const expectedInstanceId = expectedEditorPopoutInstanceIdRef.current;
        if (
          ready?.documentId === documentId
          && ready.documentEpoch === documentEpoch
        ) return;
        if (expectedInstanceId || pendingEditorPopoutReadyRequestIdRef.current) return;
        requestEditorPopoutReady();
      });
      return;
    }
    if (editorPopoutOpenRequestRef.current) return;
    const instanceId = createPaneProtocolId('markdown-media-popout');
    expectedEditorPopoutInstanceIdRef.current = instanceId;
    pendingEditorPopoutReadyRequestIdRef.current = null;
    const openRequest = openPanePopout('editor', instanceId).then((outcome) => {
      if (expectedEditorPopoutInstanceIdRef.current !== instanceId) return;
      if (outcome.status !== 'failed') editorPopoutOpenRef.current = true;
      if (outcome.status === 'existing') {
        // An existing popout owns its URL-derived instance ID, not this speculative one.
        expectedEditorPopoutInstanceIdRef.current = null;
        const ready = editorPopoutReadyRef.current;
        if (ready?.documentId === documentId && ready.documentEpoch === documentEpoch) {
          expectedEditorPopoutInstanceIdRef.current = ready.popoutInstanceId;
          startEditorPopoutHandshakeRef.current?.(ready);
        } else {
          requestEditorPopoutReady();
        }
      } else if (outcome.status === 'failed') {
        expectedEditorPopoutInstanceIdRef.current = null;
        editorPopoutOpenRef.current = false;
      }
    }).finally(() => {
      if (editorPopoutOpenRequestRef.current === openRequest) {
        editorPopoutOpenRequestRef.current = null;
      }
    });
    editorPopoutOpenRequestRef.current = openRequest;
  }, [documentEpoch, documentId, editorPopoutOpen, openPanePopout, requestEditorPopoutReady]);

  const handleWorkspaceAssetInsert = useCallback((
    asset: WorkspaceFileEntry,
    target: MarkdownMediaInsertionTarget,
  ) => {
    if (
      activeFileKind !== 'markdown'
      || authorityStatus !== 'committed'
      || !activeWorkspaceMarkdownFile
    ) return;
    const markdown = createMarkdownMediaReference(asset, activeWorkspaceMarkdownFile);
    if (!markdown) return;
    mediaInsertionRequestIdRef.current += 1;
    const insertion: MarkdownMediaInsertion = {
      documentEpoch,
      documentId,
      markdown,
      requestId: mediaInsertionRequestIdRef.current,
      target,
    };
    if (target.kind === 'cursor' && editorPopoutOpenRef.current) {
      const popoutInsertion: PendingMarkdownMediaCursorInsertion = {
        asset: {
          kind: asset.kind,
          name: asset.name,
          relative_path: asset.relative_path,
        },
        documentRelativePath: activeWorkspaceMarkdownFile.relative_path,
        documentEpoch,
        documentId,
        requestId: insertion.requestId,
      };
      const ready = editorPopoutReadyRef.current;
      if (ready?.documentId === documentId && ready.documentEpoch === documentEpoch) {
        sendCursorInsertionToEditorPopout({
          ...popoutInsertion,
          popoutInstanceId: ready.popoutInstanceId,
        });
      } else {
        pendingPopoutMediaInsertionsRef.current.push(popoutInsertion);
        const expectedInstanceId = expectedEditorPopoutInstanceIdRef.current;
        if (expectedInstanceId) {
          startEditorPopoutHandshakeRef.current?.({
            documentEpoch,
            documentId,
            popoutInstanceId: expectedInstanceId,
          });
        } else if (!pendingEditorPopoutReadyRequestIdRef.current) {
          requestEditorPopoutReady();
        }
      }
      return;
    }
    setMediaInsertion(insertion);
  }, [
    activeFileKind,
    activeWorkspaceMarkdownFile,
    authorityStatus,
    documentEpoch,
    documentId,
    requestEditorPopoutReady,
    sendCursorInsertionToEditorPopout,
  ]);

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
            : <EditorPane activePath={activePath} content={content} documentEpoch={documentEpoch} documentId={documentId} editable={authorityStatus === 'committed'} fileKind={editorFileKind} mediaInsertion={currentMediaInsertion} outlineJump={currentOutlineJump} onContentChange={updateContent} popout />}
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
            onPopout={handleEditorPopoutOpen}
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
              onPopout={handleEditorPopoutOpen}
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
