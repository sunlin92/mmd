import { useEffect, useRef, useState, type Ref } from 'react';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { html } from '@codemirror/lang-html';
import { markdown } from '@codemirror/lang-markdown';
import { defaultHighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { search, searchKeymap } from '@codemirror/search';
import { Annotation, Compartment, countColumn, EditorState, Transaction, type Extension } from '@codemirror/state';
import { drawSelection, EditorView, keymap, lineNumbers } from '@codemirror/view';
import { tagHighlighter, tags } from '@lezer/highlight';
import { vim } from '@replit/codemirror-vim';
import { applyMarkdownFormatCommand, type MarkdownFormatCommandId } from '../lib/markdownFormatCommands';
import type { MarkdownOutlineJump } from '../lib/markdownOutline';
import type { MarkdownMediaInsertion } from '../lib/markdownMedia';
import type { PanePopoutButtonState } from '../lib/paneLayout';
import { displayName } from '../lib/documentNames';
import { getEditorDocumentStats, type EditorDocumentStats } from '../lib/editorStatus';
import { MarkdownFormatDialog } from './MarkdownFormatDialog';
import { PaneHeader } from './PaneHeader';
import { VimLogo } from './VimLogo';
import type { WorkspaceFileKind } from '../types';
import { useI18n } from '../lib/i18n';

interface EditorPaneProps {
  activePath: string | null;
  content: string;
  documentEpoch: number;
  documentId: string;
  editable?: boolean;
  fileKind?: Extract<WorkspaceFileKind, 'markdown' | 'html'>;
  mediaInsertion?: MarkdownMediaInsertion | null;
  onContentChange: (content: string) => void;
  outlineJump?: MarkdownOutlineJump | null;
  onPopout?: () => void;
  paneRef?: Ref<HTMLElement>;
  popoutButton?: PanePopoutButtonState;
  popout?: boolean;
}

const externalSyncAnnotation = Annotation.define<boolean>();

interface EditorStatus extends EditorDocumentStats {
  column: number;
  line: number;
}

type DeferredDocumentStatsTask = {
  id: number;
  kind: 'debounce' | 'idle';
} | null;

const DOCUMENT_STATS_DEBOUNCE_MS = 120;
const DOCUMENT_STATS_IDLE_TIMEOUT_MS = 250;

const sourceSyntaxHighlighter = tagHighlighter([
  { tag: tags.heading, class: 'tok-heading' },
  { tag: tags.strong, class: 'tok-strong' },
  { tag: tags.emphasis, class: 'tok-emphasis' },
  { tag: tags.link, class: 'tok-link' },
  { tag: tags.url, class: 'tok-url' },
  { tag: tags.monospace, class: 'tok-monospace' },
  { tag: tags.quote, class: 'tok-quote' },
  { tag: tags.list, class: 'tok-list' },
  { tag: tags.processingInstruction, class: 'tok-meta' },
  { tag: tags.comment, class: 'tok-comment' },
]);

function editorConfiguration(fileKind: Extract<WorkspaceFileKind, 'markdown' | 'html'>, label: string): Extension {
  return [
    fileKind === 'html' ? html() : markdown(),
    EditorView.contentAttributes.of({
      'aria-label': label,
      spellcheck: 'false',
    }),
  ];
}

function editorAccessConfiguration(editable: boolean): Extension {
  return [
    EditorState.readOnly.of(!editable),
    EditorView.editable.of(editable),
    EditorView.contentAttributes.of({ 'aria-readonly': String(!editable) }),
  ];
}

function vimModeConfiguration(enabled: boolean): Extension {
  return enabled ? [vim({ status: true }), drawSelection()] : [];
}

function getEditorCursorStatus(state: EditorState): Pick<EditorStatus, 'column' | 'line'> {
  const head = state.selection.main.head;
  const line = state.doc.lineAt(head);
  return {
    column: countColumn(line.text, 2, head - line.from) + 1,
    line: line.number,
  };
}

function getEditorStatus(state: EditorState): EditorStatus {
  return {
    ...getEditorDocumentStats(state.doc.toString()),
    ...getEditorCursorStatus(state),
  };
}

function isSameEditorStatus(current: EditorStatus, next: EditorStatus): boolean {
  return current.characters === next.characters
    && current.column === next.column
    && current.line === next.line
    && current.lines === next.lines
    && current.words === next.words;
}

export function EditorPane({ activePath, content, documentEpoch, documentId, editable = true, fileKind = 'markdown', mediaInsertion, onContentChange, outlineJump, onPopout, paneRef, popoutButton, popout = false }: EditorPaneProps) {
  const { t } = useI18n();
  const editorLabel = fileKind === 'html' ? t('htmlSourceEditor') : t('markdownSourceEditor');
  const [vimModeEnabled, setVimModeEnabled] = useState(false);
  const [formatDialogOpen, setFormatDialogOpen] = useState(false);
  const [editorStatus, setEditorStatus] = useState<EditorStatus>(() => ({
    ...getEditorDocumentStats(content),
    column: 1,
    line: 1,
  }));
  const editorHostRef = useRef<HTMLDivElement>(null);
  const editorViewRef = useRef<EditorView | null>(null);
  const configurationCompartmentRef = useRef<Compartment | null>(null);
  const accessCompartmentRef = useRef<Compartment | null>(null);
  const vimModeCompartmentRef = useRef<Compartment | null>(null);
  const configuredFileKindRef = useRef(fileKind);
  const configuredEditorLabelRef = useRef(editorLabel);
  const configuredEditableRef = useRef(editable);
  const configuredVimModeRef = useRef(vimModeEnabled);
  const contentRef = useRef(content);
  const editableRef = useRef(editable);
  const fileKindRef = useRef(fileKind);
  const vimModeEnabledRef = useRef(vimModeEnabled);
  const onContentChangeRef = useRef(onContentChange);
  const lastHandledMediaInsertionRef = useRef<string | null>(null);
  contentRef.current = content;
  editableRef.current = editable;
  fileKindRef.current = fileKind;
  vimModeEnabledRef.current = vimModeEnabled;
  onContentChangeRef.current = onContentChange;

  useEffect(() => {
    const host = editorHostRef.current;
    if (!host) return undefined;

    const configurationCompartment = new Compartment();
    const accessCompartment = new Compartment();
    const vimModeCompartment = new Compartment();
    const initialFileKind = fileKindRef.current;
    const initialEditorLabel = configuredEditorLabelRef.current;
    const initialEditable = editableRef.current;
    const initialVimMode = vimModeEnabledRef.current;
    let deferredDocumentStatsTask: DeferredDocumentStatsTask = null;
    let documentStatsVersion = 0;
    let pendingDocumentState: EditorState | null = null;
    let disposed = false;
    const cancelDeferredDocumentStats = () => {
      const task = deferredDocumentStatsTask;
      if (!task) return;
      if (task.kind === 'idle') {
        if (typeof cancelIdleCallback === 'function') cancelIdleCallback(task.id);
      } else {
        window.clearTimeout(task.id);
      }
      deferredDocumentStatsTask = null;
    };
    const syncEditorCursorStatus = (state: EditorState) => {
      const cursor = getEditorCursorStatus(state);
      setEditorStatus((current) => {
        const next = { ...current, ...cursor };
        return isSameEditorStatus(current, next) ? current : next;
      });
    };
    const commitDocumentStats = (version: number) => {
      if (disposed || version !== documentStatsVersion) return;
      deferredDocumentStatsTask = null;
      const state = pendingDocumentState;
      pendingDocumentState = null;
      if (!state) return;
      const documentStats = getEditorDocumentStats(state.doc.toString());
      setEditorStatus((current) => {
        const next = { ...current, ...documentStats };
        return isSameEditorStatus(current, next) ? current : next;
      });
    };
    const scheduleDocumentStats = (state: EditorState) => {
      pendingDocumentState = state;
      cancelDeferredDocumentStats();
      documentStatsVersion += 1;
      const version = documentStatsVersion;
      const requestIdleWork = () => {
        if (disposed || version !== documentStatsVersion) return;
        deferredDocumentStatsTask = null;
        if (typeof requestIdleCallback === 'function') {
          const id = requestIdleCallback(
            () => commitDocumentStats(version),
            { timeout: DOCUMENT_STATS_IDLE_TIMEOUT_MS },
          );
          deferredDocumentStatsTask = { id, kind: 'idle' };
          return;
        }
        commitDocumentStats(version);
      };
      const id = window.setTimeout(requestIdleWork, DOCUMENT_STATS_DEBOUNCE_MS);
      deferredDocumentStatsTask = { id, kind: 'debounce' };
    };
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: contentRef.current,
        extensions: [
          vimModeCompartment.of(vimModeConfiguration(initialVimMode)),
          lineNumbers(),
          history(),
          search(),
          syntaxHighlighting(defaultHighlightStyle),
          syntaxHighlighting(sourceSyntaxHighlighter),
          EditorView.lineWrapping,
          keymap.of([
            {
              key: 'Ctrl-/',
              run: () => {
                if (!editableRef.current || fileKindRef.current !== 'markdown') return false;
                setFormatDialogOpen(true);
                return true;
              },
            },
            ...defaultKeymap,
            ...historyKeymap,
            ...searchKeymap,
          ]),
          configurationCompartment.of(editorConfiguration(initialFileKind, initialEditorLabel)),
          accessCompartment.of(editorAccessConfiguration(initialEditable)),
          EditorState.transactionFilter.of((transaction) => (
            transaction.docChanged
              && transaction.startState.facet(EditorState.readOnly)
              && !transaction.annotation(externalSyncAnnotation)
              ? []
              : transaction
          )),
          EditorView.updateListener.of((update) => {
            if (update.docChanged || update.selectionSet) {
              syncEditorCursorStatus(update.state);
            }
            if (update.docChanged) scheduleDocumentStats(update.state);
            const hasUserDocumentChange = update.transactions.some((transaction) => (
              transaction.docChanged && !transaction.annotation(externalSyncAnnotation)
            ));
            if (hasUserDocumentChange) {
              onContentChangeRef.current(update.state.doc.toString());
            }
          }),
        ],
      }),
    });
    editorViewRef.current = view;
    configurationCompartmentRef.current = configurationCompartment;
    accessCompartmentRef.current = accessCompartment;
    vimModeCompartmentRef.current = vimModeCompartment;
    configuredFileKindRef.current = initialFileKind;
    configuredEditorLabelRef.current = initialEditorLabel;
    configuredEditableRef.current = initialEditable;
    configuredVimModeRef.current = initialVimMode;
    setEditorStatus(getEditorStatus(view.state));

    return () => {
      disposed = true;
      documentStatsVersion += 1;
      pendingDocumentState = null;
      cancelDeferredDocumentStats();
      if (editorViewRef.current === view) editorViewRef.current = null;
      if (configurationCompartmentRef.current === configurationCompartment) {
        configurationCompartmentRef.current = null;
      }
      if (accessCompartmentRef.current === accessCompartment) {
        accessCompartmentRef.current = null;
      }
      if (vimModeCompartmentRef.current === vimModeCompartment) {
        vimModeCompartmentRef.current = null;
      }
      view.destroy();
    };
  }, [documentEpoch, documentId]);

  useEffect(() => {
    const view = editorViewRef.current;
    const accessCompartment = accessCompartmentRef.current;
    if (!view || !accessCompartment || configuredEditableRef.current === editable) return;
    view.dispatch({ effects: accessCompartment.reconfigure(editorAccessConfiguration(editable)) });
    configuredEditableRef.current = editable;
  }, [documentEpoch, documentId, editable]);

  useEffect(() => {
    const view = editorViewRef.current;
    const vimModeCompartment = vimModeCompartmentRef.current;
    if (!view || !vimModeCompartment || configuredVimModeRef.current === vimModeEnabled) return;
    view.dispatch({ effects: vimModeCompartment.reconfigure(vimModeConfiguration(vimModeEnabled)) });
    configuredVimModeRef.current = vimModeEnabled;
    view.focus();
  }, [documentEpoch, documentId, vimModeEnabled]);

  useEffect(() => {
    const view = editorViewRef.current;
    if (!view || view.state.doc.toString() === content) return;
    view.dispatch({
      annotations: [
        externalSyncAnnotation.of(true),
        Transaction.addToHistory.of(false),
      ],
      changes: { from: 0, to: view.state.doc.length, insert: content },
    });
  }, [content, documentEpoch, documentId]);

  useEffect(() => {
    const view = editorViewRef.current;
    if (
      !view
      || !mediaInsertion
      || mediaInsertion.documentId !== documentId
      || mediaInsertion.documentEpoch !== documentEpoch
      || !editableRef.current
      || fileKindRef.current !== 'markdown'
      || !mediaInsertion.markdown
    ) return;
    const insertionKey = `${mediaInsertion.documentId}:${mediaInsertion.documentEpoch}:${mediaInsertion.requestId}`;
    if (lastHandledMediaInsertionRef.current === insertionKey) return;
    const position = view.posAtCoords({ x: mediaInsertion.clientX, y: mediaInsertion.clientY })
      ?? view.state.selection.main.head;
    lastHandledMediaInsertionRef.current = insertionKey;
    view.dispatch({
      changes: { from: position, insert: mediaInsertion.markdown },
      scrollIntoView: true,
      selection: { anchor: position + mediaInsertion.markdown.length },
    });
    view.focus();
  }, [documentEpoch, documentId, mediaInsertion]);

  useEffect(() => {
    const view = editorViewRef.current;
    if (
      !view
      || !outlineJump
      || outlineJump.documentId !== documentId
      || outlineJump.documentEpoch !== documentEpoch
    ) return;
    const line = Math.min(Math.max(1, outlineJump.item.line), view.state.doc.lines);
    view.dispatch({
      scrollIntoView: true,
      selection: { anchor: view.state.doc.line(line).from },
    });
  }, [documentEpoch, documentId, outlineJump]);

  useEffect(() => {
    const view = editorViewRef.current;
    const configurationCompartment = configurationCompartmentRef.current;
    if (!view || !configurationCompartment || (configuredFileKindRef.current === fileKind && configuredEditorLabelRef.current === editorLabel)) return;
    view.dispatch({ effects: configurationCompartment.reconfigure(editorConfiguration(fileKind, editorLabel)) });
    configuredFileKindRef.current = fileKind;
    configuredEditorLabelRef.current = editorLabel;
  }, [documentEpoch, documentId, editorLabel, fileKind]);

  useEffect(() => {
    if (editable && fileKind === 'markdown') return;
    setFormatDialogOpen(false);
  }, [documentEpoch, documentId, editable, fileKind]);

  const closeFormatDialog = () => {
    setFormatDialogOpen(false);
    editorViewRef.current?.focus();
  };

  const applyFormatCommand = (command: MarkdownFormatCommandId) => {
    const view = editorViewRef.current;
    if (!view || !editableRef.current || fileKindRef.current !== 'markdown') {
      setFormatDialogOpen(false);
      return;
    }
    const edit = applyMarkdownFormatCommand(
      view.state.doc.toString(),
      view.state.selection.main,
      command,
    );
    view.dispatch({
      changes: { from: edit.from, insert: edit.insert, to: edit.to },
      scrollIntoView: true,
      selection: edit.selection,
    });
    setFormatDialogOpen(false);
    view.focus();
  };

  return (
    <section className={popout ? 'editor-pane popout-pane' : 'editor-pane'} ref={paneRef}>
      <PaneHeader
        title={t('editor')}
        subtitle={displayName(activePath)}
        beforePopout={(
          <button
            type="button"
            className={vimModeEnabled ? 'pane-vim-button is-active' : 'pane-vim-button'}
            title={vimModeEnabled ? t('disableVim') : t('enableVim')}
            aria-label={vimModeEnabled ? t('disableVim') : t('enableVim')}
            aria-pressed={vimModeEnabled}
            onClick={() => setVimModeEnabled((enabled) => !enabled)}
          >
            <VimLogo className="vim-logo" />
          </button>
        )}
        popoutButton={popoutButton}
        onPopout={onPopout}
      />
      <div
        aria-label={editorLabel}
        className="editor-host"
        data-markdown-media-drop-target={editable && fileKind === 'markdown' ? 'true' : undefined}
        ref={editorHostRef}
      />
      <footer className="editor-status" aria-label={t('editorStatus')}>
        <span className="editor-status-stat editor-status-words">{t('words', { count: editorStatus.words })}</span>
        <span className="editor-status-stat editor-status-characters">{t('characters', { count: editorStatus.characters })}</span>
        <span className="editor-status-stat editor-status-lines">{t('lines', { count: editorStatus.lines })}</span>
        <span className="editor-status-cursor">{t('lineColumn', { line: editorStatus.line, column: editorStatus.column })}</span>
      </footer>
      {formatDialogOpen && (
        <MarkdownFormatDialog
          onCancel={closeFormatDialog}
          onSelect={applyFormatCommand}
        />
      )}
    </section>
  );
}
