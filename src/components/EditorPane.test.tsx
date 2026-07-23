// @vitest-environment jsdom

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { redo, undo, undoDepth } from '@codemirror/commands';
import { htmlLanguage } from '@codemirror/lang-html';
import { markdownLanguage } from '@codemirror/lang-markdown';
import { highlightingFor } from '@codemirror/language';
import {
  findNext,
  findPrevious,
  replaceAll,
  replaceNext,
  SearchQuery,
  setSearchQuery,
} from '@codemirror/search';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { tags } from '@lezer/highlight';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getPanePopoutButtonState } from '../lib/paneLayout';
import { applyEffectiveTheme, SKIN_IDS } from '../lib/theme';
import { EditorPane } from './EditorPane';

if (typeof Range.prototype.getClientRects !== 'function') {
  Object.defineProperty(Range.prototype, 'getClientRects', {
    configurable: true,
    value: () => [],
  });
}

if (typeof Range.prototype.getBoundingClientRect !== 'function') {
  Object.defineProperty(Range.prototype, 'getBoundingClientRect', {
    configurable: true,
    value: () => new DOMRect(),
  });
}

if (typeof document.elementFromPoint !== 'function') {
  Object.defineProperty(document, 'elementFromPoint', {
    configurable: true,
    value: () => null,
  });
}

describe('EditorPane', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('renders a native CodeMirror editor with line numbers', () => {
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Notes"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={vi.fn<(content: string) => void>()}
        />,
      );
    });

    expect(container.querySelector('textarea')).toBeNull();
    expect(container.querySelector('.cm-editor')).not.toBeNull();
    expect(container.querySelector('.cm-gutters')).not.toBeNull();
    expect(container.querySelector('.cm-content')?.classList.contains('cm-lineWrapping'))
      .toBe(true);
    expect(container.querySelector('[role="textbox"]')?.getAttribute('aria-label'))
      .toBe('Markdown source editor');
  });

  it('preserves the active editor session while every application skin is applied', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    const documentId = 'document-unsaved-theme-check';
    act(() => {
      root.render(
        <section data-dirty="true" data-document-id={documentId}>
          <EditorPane
            activePath="/workspace/unsaved.md"
            content={'# Unsaved\n\nKeep this draft.'}
            documentEpoch={9}
            documentId={documentId}
            onContentChange={onContentChange}
          />
        </section>,
      );
    });
    const sessionElement = container.querySelector<HTMLElement>('[data-document-id]');
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const initialView = editor ? EditorView.findFromDOM(editor) : null;
    if (!initialView || !sessionElement) throw new Error('Expected active editor session');

    act(() => {
      initialView.dispatch({ selection: { anchor: 12 } });
      initialView.scrollDOM.scrollTop = 37;
    });

    for (const skin of SKIN_IDS) {
      applyEffectiveTheme(document.documentElement, {
        appearance: skin === 'shanshui-yemo' ? 'dark' : 'light',
        skin,
      });
    }

    const currentEditor = container.querySelector<HTMLElement>('.cm-editor');
    const currentView = currentEditor ? EditorView.findFromDOM(currentEditor) : null;
    expect(currentView).toBe(initialView);
    expect(currentView?.state.doc.toString()).toBe('# Unsaved\n\nKeep this draft.');
    expect(currentView?.state.selection.main.head).toBe(12);
    expect(currentView?.scrollDOM.scrollTop).toBe(37);
    expect(sessionElement.dataset.documentId).toBe(documentId);
    expect(sessionElement.dataset.dirty).toBe('true');
    expect(onContentChange).not.toHaveBeenCalled();
  });

  it('shows live document statistics and the current cursor location', () => {
    const content = 'Hello 世界\nnext';
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content={content}
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');
    const status = container.querySelector<HTMLElement>('[aria-label="Editor status"]');

    expect(status?.textContent).toContain('Words 4');
    expect(status?.textContent).toContain('Characters 13');
    expect(status?.textContent).toContain('Lines 2');
    expect(status?.textContent).toContain('Line 1, Column 1');

    act(() => view.dispatch({ selection: { anchor: 11 } }));
    expect(status?.textContent).toContain('Line 2, Column 3');

    const idleCallbacks = new Map<number, IdleRequestCallback>();
    const cancelIdleCallback = vi.fn<(id: number) => void>((id) => {
      idleCallbacks.delete(id);
    });
    let nextIdleCallbackId = 1;
    vi.useFakeTimers();
    vi.stubGlobal('requestIdleCallback', (callback: IdleRequestCallback) => {
      const id = nextIdleCallbackId;
      nextIdleCallbackId += 1;
      idleCallbacks.set(id, callback);
      return id;
    });
    vi.stubGlobal('cancelIdleCallback', cancelIdleCallback);

    act(() => view.dispatch({ changes: { from: view.state.doc.length, insert: '!' } }));
    act(() => view.dispatch({ changes: { from: view.state.doc.length, insert: '?' } }));

    expect(status?.textContent).toContain('Characters 13');
    expect(cancelIdleCallback).not.toHaveBeenCalled();
    expect(idleCallbacks.size).toBe(0);

    act(() => vi.advanceTimersByTime(120));
    expect([...idleCallbacks.keys()]).toEqual([1]);

    const callback = idleCallbacks.get(1);
    if (!callback) throw new Error('Expected document statistics idle callback');
    act(() => callback({ didTimeout: false, timeRemaining: () => 50 }));

    expect(status?.textContent).toContain('Characters 15');
  });

  it('reports the visual cursor column when a line contains a tab', () => {
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content={'\titem'}
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');
    const status = container.querySelector<HTMLElement>('[aria-label="Editor status"]');

    act(() => view.dispatch({ selection: { anchor: 1 } }));
    expect(status?.textContent).toContain('Line 1, Column 3');
  });

  it('refreshes document statistics after an external content update', () => {
    const idleCallbacks = new Map<number, IdleRequestCallback>();
    let nextIdleCallbackId = 1;
    vi.useFakeTimers();
    vi.stubGlobal('requestIdleCallback', (callback: IdleRequestCallback) => {
      const id = nextIdleCallbackId;
      nextIdleCallbackId += 1;
      idleCallbacks.set(id, callback);
      return id;
    });
    vi.stubGlobal('cancelIdleCallback', (id: number) => idleCallbacks.delete(id));
    const renderEditor = (content: string) => (
      <EditorPane
        activePath="/workspace/notes.md"
        content={content}
        documentEpoch={1}
        documentId="document-notes"
        onContentChange={vi.fn<(value: string) => void>()}
      />
    );
    act(() => root.render(renderEditor('one')));
    const status = container.querySelector<HTMLElement>('[aria-label="Editor status"]');
    expect(status?.textContent).toContain('Words 1');
    expect(status?.textContent).toContain('Characters 3');

    act(() => root.render(renderEditor('one\ntwo')));
    expect(status?.textContent).toContain('Words 1');
    act(() => vi.advanceTimersByTime(120));
    const callback = idleCallbacks.get(1);
    if (!callback) throw new Error('Expected external document statistics idle callback');
    act(() => callback({ didTimeout: false, timeRemaining: () => 50 }));

    expect(status?.textContent).toContain('Words 2');
    expect(status?.textContent).toContain('Characters 7');
    expect(status?.textContent).toContain('Lines 2');
  });

  it('does not apply deferred statistics from a replaced document', () => {
    const idleCallbacks = new Map<number, IdleRequestCallback>();
    let nextIdleCallbackId = 1;
    vi.useFakeTimers();
    vi.stubGlobal('requestIdleCallback', (callback: IdleRequestCallback) => {
      const id = nextIdleCallbackId;
      nextIdleCallbackId += 1;
      idleCallbacks.set(id, callback);
      return id;
    });
    vi.stubGlobal('cancelIdleCallback', vi.fn<(id: number) => void>());

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/old.md"
          content="old"
          documentEpoch={1}
          documentId="document-old"
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => view.dispatch({ changes: { from: view.state.doc.length, insert: '!' } }));
    act(() => vi.advanceTimersByTime(120));
    const staleCallback = idleCallbacks.get(1);
    if (!staleCallback) throw new Error('Expected old document statistics idle callback');

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/new.md"
          content="新"
          documentEpoch={2}
          documentId="document-new"
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    const status = container.querySelector<HTMLElement>('[aria-label="Editor status"]');
    expect(status?.textContent).toContain('Characters 1');

    act(() => staleCallback({ didTimeout: false, timeRemaining: () => 50 }));

    expect(status?.textContent).toContain('Words 1');
    expect(status?.textContent).toContain('Characters 1');
    expect(status?.textContent).toContain('Lines 1');
  });

  it('moves to an outline heading and requests CodeMirror scrolling', () => {
    const content = '# Project\n\n## Install';
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content={content}
          documentEpoch={1}
          documentId="document-guide"
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');
    const dispatch = vi.spyOn(view, 'dispatch');

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content={content}
          documentEpoch={1}
          documentId="document-guide"
          outlineJump={{
            documentId: 'document-guide',
            documentEpoch: 1,
            item: {
              depth: 1,
              id: 'heading-11',
              level: 2,
              line: 3,
              offset: 11,
              ordinal: 1,
              text: 'Install',
            },
            requestId: 1,
          }}
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });

    expect(view.state.selection.main.head).toBe(11);
    expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({
      scrollIntoView: true,
      selection: { anchor: 11 },
    }));
  });

  it('uses the heading line instead of a CRLF source offset', () => {
    const content = '# Project\r\n\r\n## Install';
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content={content}
          documentEpoch={1}
          documentId="document-guide"
          outlineJump={{
            documentId: 'document-guide',
            documentEpoch: 1,
            item: {
              depth: 1,
              id: 'heading-13',
              level: 2,
              line: 3,
              offset: 13,
              ordinal: 1,
              text: 'Install',
            },
            requestId: 2,
          }}
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;

    expect(view?.state.doc.toString()).toBe('# Project\n\n## Install');
    expect(view?.state.selection.main.head).toBe(11);
  });

  it('inserts a dropped media reference at the release position as one undoable edit', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    const content = 'before after';
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content={content}
          documentEpoch={1}
          documentId="document-guide"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');
    const posAtCoords = vi.spyOn(view, 'posAtCoords').mockReturnValue(7);

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content={content}
          documentEpoch={1}
          documentId="document-guide"
          mediaInsertion={{
            documentEpoch: 1,
            documentId: 'document-guide',
            markdown: '![cover.png](assets/cover.png)',
            requestId: 1,
            target: { kind: 'coordinates', clientX: 120, clientY: 80 },
          }}
          onContentChange={onContentChange}
        />,
      );
    });

    expect(posAtCoords).toHaveBeenCalledWith({ x: 120, y: 80 });
    expect(view.state.doc.toString()).toBe('before ![cover.png](assets/cover.png)after');
    expect(view.state.selection.main.head).toBe(37);
    expect(onContentChange).toHaveBeenCalledWith('before ![cover.png](assets/cover.png)after');

    act(() => undo(view));
    expect(view.state.doc.toString()).toBe(content);
  });

  it('inserts a context-menu media reference at the current cursor', () => {
    const content = 'before after';
    const onContentChange = vi.fn<(nextContent: string) => void>();
    act(() => root.render(
      <EditorPane
        activePath="/workspace/guide.md"
        content={content}
        documentEpoch={1}
        documentId="document-guide"
        onContentChange={onContentChange}
      />,
    ));
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');
    act(() => view.dispatch({ selection: { anchor: 7 } }));
    const posAtCoords = vi.spyOn(view, 'posAtCoords');

    act(() => root.render(
      <EditorPane
        activePath="/workspace/guide.md"
        content={content}
        documentEpoch={1}
        documentId="document-guide"
        mediaInsertion={{
          documentEpoch: 1,
          documentId: 'document-guide',
          markdown: '[intro.mp3](audio/intro.mp3)',
          requestId: 1,
          target: { kind: 'cursor' },
        }}
        onContentChange={onContentChange}
      />,
    ));

    expect(posAtCoords).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe('before [intro.mp3](audio/intro.mp3)after');
    expect(onContentChange).toHaveBeenCalledWith('before [intro.mp3](audio/intro.mp3)after');

    act(() => undo(view));
    expect(view.state.doc.toString()).toBe(content);
  });

  it('does not insert dropped media into a read-only editor', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content="protected"
          documentEpoch={1}
          documentId="document-guide"
          editable={false}
          mediaInsertion={{
            documentEpoch: 1,
            documentId: 'document-guide',
            markdown: '![cover.png](assets/cover.png)',
            requestId: 1,
            target: { kind: 'coordinates', clientX: 120, clientY: 80 },
          }}
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;

    expect(view?.state.doc.toString()).toBe('protected');
    expect(onContentChange).not.toHaveBeenCalled();
  });

  it('ignores an outline jump from a previous document epoch', () => {
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content="# Replacement\n\n## Install"
          documentEpoch={2}
          documentId="document-guide"
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/guide.md"
          content="# Replacement\n\n## Install"
          documentEpoch={2}
          documentId="document-guide"
          outlineJump={{
            documentId: 'document-guide',
            documentEpoch: 1,
            item: {
              depth: 1,
              id: 'heading-11',
              level: 2,
              line: 3,
              offset: 11,
              ordinal: 1,
              text: 'Install',
            },
            requestId: 3,
          }}
          onContentChange={vi.fn<(value: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;

    expect(view?.state.selection.main.head).toBe(0);
  });

  it('toggles Vim editing mode from the button immediately before pop out', () => {
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Notes"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={vi.fn<(content: string) => void>()}
          onPopout={vi.fn<() => void>()}
          popoutButton={getPanePopoutButtonState('editor', false)}
        />,
      );
    });

    const vimButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Enable Vim editing mode"]',
    );
    const popoutButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Pop out editor"]',
    );

    expect(vimButton).not.toBeNull();
    expect(vimButton?.getAttribute('aria-pressed')).toBe('false');
    expect(vimButton?.querySelector('svg.vim-logo')).not.toBeNull();
    expect(vimButton?.nextElementSibling).toBe(popoutButton);

    act(() => {
      vimButton?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    const activeVimButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Disable Vim editing mode"]',
    );
    expect(activeVimButton?.getAttribute('aria-pressed')).toBe('true');
  });

  it('handles Vim normal-mode commands after Vim editing mode is enabled', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="alpha"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    const vimButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Enable Vim editing mode"]',
    );
    if (!view || !vimButton) throw new Error('Expected editor and Vim mode button');

    act(() => {
      vimButton.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        key: 'x',
      }));
    });

    expect(view.state.doc.toString()).toBe('lpha');
    expect(onContentChange).toHaveBeenCalledOnce();
    expect(onContentChange).toHaveBeenCalledWith('lpha');
  });

  it('opens the Markdown format command dialog with Control slash', () => {
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="alpha"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={vi.fn<(content: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        ctrlKey: true,
        key: '/',
      }));
    });

    expect(container.querySelector('dialog[aria-modal="true"]')).not.toBeNull();
    expect(container.querySelector('[role="menu"]')).not.toBeNull();
    expect(container.querySelector('[role="menuitem"][data-command-id="h1"]')?.textContent)
      .toContain('Heading 1');
    expect(container.querySelector('[role="menuitem"][data-command-id="alert-error"]')?.textContent)
      .toContain('Error');
  });

  it('chooses format commands with arrow keys and Enter', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content=""
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        ctrlKey: true,
        key: '/',
      }));
    });
    const search = container.querySelector<HTMLInputElement>('[role="combobox"]');
    act(() => {
      search?.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'ArrowDown' }));
    });
    act(() => {
      search?.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Enter' }));
    });

    expect(view.state.doc.toString()).toBe('## ');
    expect(view.state.selection.main.head).toBe(3);
    expect(onContentChange).toHaveBeenCalledWith('## ');
  });

  it('wraps the current selection with the chosen format command', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="alpha beta"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => view.dispatch({ selection: { anchor: 0, head: 5 } }));
    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        ctrlKey: true,
        key: '/',
      }));
    });
    act(() => {
      container.querySelector<HTMLButtonElement>('[data-command-id="bold"]')?.click();
    });

    expect(view.state.doc.toString()).toBe('**alpha** beta');
    expect(view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to))
      .toBe('alpha');
    expect(onContentChange).toHaveBeenCalledOnce();
    expect(onContentChange).toHaveBeenCalledWith('**alpha** beta');
    expect(container.querySelector('[role="dialog"]')).toBeNull();
  });

  it('inserts an empty alert template at the caret', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="before after"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => view.dispatch({ selection: { anchor: 7 } }));
    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        ctrlKey: true,
        key: '/',
      }));
    });
    act(() => {
      container.querySelector<HTMLButtonElement>('[data-command-id="alert-tip"]')?.click();
    });

    expect(view.state.doc.toString()).toBe('before \n> [!TIP]\n> \nafter');
    expect(view.state.selection.main.head).toBe(19);
    expect(onContentChange).toHaveBeenCalledWith('before \n> [!TIP]\n> \nafter');
  });

  it('closes the format command dialog with Escape without changing content', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="alpha"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        ctrlKey: true,
        key: '/',
      }));
    });
    const search = container.querySelector<HTMLInputElement>('[role="combobox"]');
    act(() => {
      search?.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }));
    });

    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(view.state.doc.toString()).toBe('alpha');
    expect(onContentChange).not.toHaveBeenCalled();
  });

  it.each([
    { editable: false, fileKind: 'markdown' as const },
    { editable: true, fileKind: 'html' as const },
  ])('does not open Markdown formats for $fileKind with editable=$editable', ({ editable, fileKind }) => {
    act(() => {
      root.render(
        <EditorPane
          activePath={fileKind === 'html' ? '/workspace/index.html' : '/workspace/notes.md'}
          content="alpha"
          documentEpoch={1}
          documentId="document-notes"
          editable={editable}
          fileKind={fileKind}
          onContentChange={vi.fn<(content: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        ctrlKey: true,
        key: '/',
      }));
    });

    expect(container.querySelector('[role="dialog"]')).toBeNull();
  });

  it('renders distinct syntax highlighting for common Markdown constructs', () => {
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content={'# Heading\n\n**Strong** and *emphasis* with [link](https://example.com) and `code`.'}
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={vi.fn<(content: string) => void>()}
        />,
      );
    });

    const highlightedText = (selector: string) => Array.from(
      container.querySelectorAll<HTMLElement>(selector),
      (element) => element.textContent ?? '',
    ).join('');

    expect(highlightedText('.tok-heading')).toContain('Heading');
    expect(highlightedText('.tok-strong')).toContain('Strong');
    expect(highlightedText('.tok-emphasis')).toContain('emphasis');
    expect(highlightedText('.tok-link')).toContain('link');
    expect(highlightedText('.tok-url')).toContain('https://example.com');
    expect(highlightedText('.tok-monospace')).toContain('code');
  });

  it('renders syntax-highlighted HTML with the HTML language active', () => {
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/index.html"
          content="<h1>Home</h1>"
          documentEpoch={1}
          documentId="document-html"
          fileKind="html"
          onContentChange={vi.fn<(content: string) => void>()}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;

    expect(view ? htmlLanguage.isActiveAt(view.state, 0) : false).toBe(true);
    expect(view ? highlightingFor(view.state, [tags.tagName]) : null).not.toBeNull();
  });

  it('emits the complete editor content exactly once for one user change', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Before"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;

    expect(view?.state.doc.toString()).toBe('# Before');

    act(() => {
      if (!view) throw new Error('Expected CodeMirror editor');
      view.dispatch({
        changes: { from: view.state.doc.length, insert: ' edited' },
      });
    });

    expect(onContentChange).toHaveBeenCalledOnce();
    expect(onContentChange).toHaveBeenCalledWith('# Before edited');
  });

  it('rejects document changes while the editor is read-only', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Protected"
          documentEpoch={1}
          documentId="document-protected"
          editable={false}
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    expect(view.state.facet(EditorView.editable)).toBe(false);
    expect(view.state.facet(EditorState.readOnly)).toBe(true);
    expect(view.contentDOM.getAttribute('aria-readonly')).toBe('true');

    act(() => {
      view.dispatch({ changes: { from: view.state.doc.length, insert: ' changed' } });
    });

    expect(view.state.doc.toString()).toBe('# Protected');
    expect(onContentChange).not.toHaveBeenCalled();
  });

  it('reconfigures editability without recreating the editor', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Protected"
          documentEpoch={1}
          documentId="document-protected"
          editable={false}
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const initialView = editor ? EditorView.findFromDOM(editor) : null;
    if (!initialView) throw new Error('Expected CodeMirror editor');

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Protected"
          documentEpoch={1}
          documentId="document-protected"
          editable
          onContentChange={onContentChange}
        />,
      );
    });
    const currentEditor = container.querySelector<HTMLElement>('.cm-editor');
    const currentView = currentEditor ? EditorView.findFromDOM(currentEditor) : null;

    expect(currentView).toBe(initialView);
    expect(currentView?.state.facet(EditorView.editable)).toBe(true);
    expect(currentView?.state.facet(EditorState.readOnly)).toBe(false);
    expect(currentView?.contentDOM.getAttribute('aria-readonly')).toBe('false');

    act(() => {
      currentView?.dispatch({
        changes: { from: currentView.state.doc.length, insert: ' changed' },
      });
    });

    expect(onContentChange).toHaveBeenCalledOnce();
    expect(onContentChange).toHaveBeenCalledWith('# Protected changed');
  });

  it('applies external content to the same document without echoing or recreating the editor', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Before"
          documentEpoch={3}
          documentId="document-current"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const initialView = editor ? EditorView.findFromDOM(editor) : null;

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Synced"
          documentEpoch={3}
          documentId="document-current"
          onContentChange={onContentChange}
        />,
      );
    });
    const currentEditor = container.querySelector<HTMLElement>('.cm-editor');
    const currentView = currentEditor ? EditorView.findFromDOM(currentEditor) : null;

    expect(currentView).toBe(initialView);
    expect(currentView?.state.doc.toString()).toBe('# Synced');
    expect(currentView ? undoDepth(currentView.state) : -1).toBe(0);
    expect(onContentChange).not.toHaveBeenCalled();
  });

  it('does not dispatch an identical external content value', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Stable"
          documentEpoch={2}
          documentId="document-stable"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');
    act(() => view.dispatch({ selection: { anchor: 3 } }));
    const previousState = view.state;

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Stable"
          documentEpoch={2}
          documentId="document-stable"
          onContentChange={onContentChange}
        />,
      );
    });

    expect(view.state).toBe(previousState);
    expect(view.state.selection.main.head).toBe(3);
    expect(onContentChange).not.toHaveBeenCalled();
  });

  it('reconfigures language for the same identity without recreating the editor', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/source"
          content="# Markdown"
          documentEpoch={7}
          documentId="document-source"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const initialView = editor ? EditorView.findFromDOM(editor) : null;

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/source"
          content="<h1>HTML</h1>"
          documentEpoch={7}
          documentId="document-source"
          fileKind="html"
          onContentChange={onContentChange}
        />,
      );
    });
    const currentEditor = container.querySelector<HTMLElement>('.cm-editor');
    const currentView = currentEditor ? EditorView.findFromDOM(currentEditor) : null;

    expect(currentView).toBe(initialView);
    expect(currentView ? htmlLanguage.isActiveAt(currentView.state, 0) : false).toBe(true);
    expect(currentView?.contentDOM.getAttribute('aria-label')).toBe('HTML source editor');
    expect(onContentChange).not.toHaveBeenCalled();
  });

  it('resets local history and language when document identity changes', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/shared-path"
          content="# First"
          documentEpoch={1}
          documentId="document-first"
          onContentChange={onContentChange}
        />,
      );
    });
    const firstEditor = container.querySelector<HTMLElement>('.cm-editor');
    const firstView = firstEditor ? EditorView.findFromDOM(firstEditor) : null;
    if (!firstView) throw new Error('Expected first CodeMirror editor');
    act(() => {
      firstView.dispatch({ changes: { from: firstView.state.doc.length, insert: ' edited' } });
    });
    expect(undoDepth(firstView.state)).toBe(1);
    expect(markdownLanguage.isActiveAt(firstView.state, 0)).toBe(true);

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/shared-path"
          content="<h1>Second</h1>"
          documentEpoch={2}
          documentId="document-second"
          fileKind="html"
          onContentChange={onContentChange}
        />,
      );
    });
    const secondEditor = container.querySelector<HTMLElement>('.cm-editor');
    const secondView = secondEditor ? EditorView.findFromDOM(secondEditor) : null;

    expect(secondView).not.toBe(firstView);
    expect(secondView?.state.doc.toString()).toBe('<h1>Second</h1>');
    expect(secondView ? undoDepth(secondView.state) : -1).toBe(0);
    expect(secondView ? htmlLanguage.isActiveAt(secondView.state, 0) : false).toBe(true);
    expect(firstView.dom.isConnected).toBe(false);
  });

  it('destroys each EditorView on identity replacement and unmount', () => {
    const destroy = vi.spyOn(EditorView.prototype, 'destroy');
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/first.md"
          content="# First"
          documentEpoch={1}
          documentId="document-first"
          onContentChange={onContentChange}
        />,
      );
    });

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/second.md"
          content="# Second"
          documentEpoch={2}
          documentId="document-second"
          onContentChange={onContentChange}
        />,
      );
    });
    expect(destroy).toHaveBeenCalledTimes(1);

    act(() => root.unmount());
    expect(destroy).toHaveBeenCalledTimes(2);
    root = createRoot(container);
  });

  it('ignores late document changes dispatched by a replaced editor view', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/first.md"
          content="# First"
          documentEpoch={1}
          documentId="document-first"
          onContentChange={onContentChange}
        />,
      );
    });
    const firstEditor = container.querySelector<HTMLElement>('.cm-editor');
    const firstView = firstEditor ? EditorView.findFromDOM(firstEditor) : null;
    if (!firstView) throw new Error('Expected first CodeMirror editor');

    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/second.md"
          content="# Second"
          documentEpoch={2}
          documentId="document-second"
          onContentChange={onContentChange}
        />,
      );
    });
    const secondEditor = container.querySelector<HTMLElement>('.cm-editor');
    const secondView = secondEditor ? EditorView.findFromDOM(secondEditor) : null;
    if (!secondView) throw new Error('Expected second CodeMirror editor');

    act(() => {
      firstView.dispatch({
        changes: { from: firstView.state.doc.length, insert: ' late' },
      });
    });

    expect(onContentChange).not.toHaveBeenCalled();
    expect(secondView.state.doc.toString()).toBe('# Second');
  });

  it('keeps native undo and redo synchronized through the content callback', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="# Before"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => {
      view.dispatch({ changes: { from: view.state.doc.length, insert: ' edited' } });
    });
    expect(onContentChange).toHaveBeenLastCalledWith('# Before edited');

    let didUndo = false;
    act(() => {
      didUndo = undo(view);
    });
    expect(didUndo).toBe(true);
    expect(onContentChange).toHaveBeenLastCalledWith('# Before');

    let didRedo = false;
    act(() => {
      didRedo = redo(view);
    });
    expect(didRedo).toBe(true);
    expect(onContentChange).toHaveBeenLastCalledWith('# Before edited');
    expect(onContentChange).toHaveBeenCalledTimes(3);
  });

  it('opens native search and applies replace commands through the content callback', () => {
    const onContentChange = vi.fn<(content: string) => void>();
    act(() => {
      root.render(
        <EditorPane
          activePath="/workspace/notes.md"
          content="alpha beta alpha"
          documentEpoch={1}
          documentId="document-notes"
          onContentChange={onContentChange}
        />,
      );
    });
    const editor = container.querySelector<HTMLElement>('.cm-editor');
    const view = editor ? EditorView.findFromDOM(editor) : null;
    if (!view) throw new Error('Expected CodeMirror editor');

    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
        bubbles: true,
        ctrlKey: true,
        key: 'f',
      }));
    });
    expect(container.querySelector('.cm-search')).not.toBeNull();

    act(() => {
      view.dispatch({
        effects: setSearchQuery.of(new SearchQuery({ search: 'alpha', replace: 'omega' })),
      });
    });

    expect(findNext(view)).toBe(true);
    expect(view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to))
      .toBe('alpha');
    expect(findPrevious(view)).toBe(true);
    expect(view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to))
      .toBe('alpha');

    act(() => {
      expect(replaceNext(view)).toBe(true);
    });
    expect(onContentChange).toHaveBeenLastCalledWith('alpha beta omega');

    act(() => {
      expect(replaceAll(view)).toBe(true);
    });
    expect(onContentChange).toHaveBeenLastCalledWith('omega beta omega');
    expect(onContentChange).toHaveBeenCalledTimes(2);
  });
});
