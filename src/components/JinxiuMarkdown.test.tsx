// @vitest-environment jsdom

import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { resetImagePreviewCache } from '../lib/workspacePreviewSource';
import JinxiuMarkdown from './JinxiuMarkdown';

const tauriMocks = vi.hoisted(() => ({
  convertFileSrc: vi.fn<(path: string) => string>((path) => `asset://localhost/${encodeURIComponent(path)}`),
  invoke: vi.fn<(command: string, payload?: unknown) => Promise<unknown>>(),
}));

const mermaidMocks = vi.hoisted(() => ({
  initialize: vi.fn<(config: unknown) => void>(),
  render: vi.fn<(id: string, source: string) => Promise<{ svg: string }>>(),
}));

vi.mock('@tauri-apps/api/core', () => tauriMocks);
vi.mock('mermaid', () => ({ default: mermaidMocks }));

describe('JinxiuMarkdown document transitions', () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    resetImagePreviewCache();
    tauriMocks.invoke.mockReset();
    tauriMocks.invoke.mockImplementation(async (_command, payload) => {
      const imageSrc = (payload as { imageSrc: string }).imageSrc;
      return `/workspace/assets/${imageSrc}`;
    });
    mermaidMocks.initialize.mockReset();
    mermaidMocks.render.mockReset();
    mermaidMocks.render.mockResolvedValue({
      svg: '<svg xmlns="http://www.w3.org/2000/svg"><text>Mermaid diagram</text></svg>',
    });
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('never resolves an old dirty document image against the next document path', async () => {
    await act(async () => root.render(
      <JinxiuMarkdown
        currentFilePath="/workspace/old/index.md"
        workspaceRoot="/workspace"
      >
        {'![old](old-image.png)'}
      </JinxiuMarkdown>,
    ));
    tauriMocks.invoke.mockClear();

    await act(async () => root.render(
      <JinxiuMarkdown
        currentFilePath="/workspace/new/index.md"
        workspaceRoot="/workspace"
      >
        {'![new](new-image.png)'}
      </JinxiuMarkdown>,
    ));

    expect(tauriMocks.invoke).not.toHaveBeenCalledWith('resolve_markdown_image', {
      currentFilePath: '/workspace/new/index.md',
      imageSrc: 'old-image.png',
      workspaceRoot: '/workspace',
    });
    expect(tauriMocks.invoke).toHaveBeenCalledWith('resolve_markdown_image', {
      currentFilePath: '/workspace/new/index.md',
      imageSrc: 'new-image.png',
      workspaceRoot: '/workspace',
    });
  });

  it('keeps each rendered heading linked to its Markdown source line', async () => {
    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {'# Project\n\n## Install'}
      </JinxiuMarkdown>,
    ));

    expect(container.querySelector('h1')?.getAttribute('data-heading-line')).toBe('1');
    expect(container.querySelector('h2')?.getAttribute('data-heading-line')).toBe('3');
  });

  it('renders math with the stylesheet-compatible KaTeX markup', async () => {
    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {'Inline $x^2$\n\n$$\n\\frac{1}{\\sqrt{2}}\n$$'}
      </JinxiuMarkdown>,
    ));

    const renderedMath = [...container.querySelectorAll('.katex')];
    expect(renderedMath).toHaveLength(2);
    expect(renderedMath.every((element) => (
      element.querySelector('.katex-html > .katex-base .katex-strut') !== null
    ))).toBe(true);
    expect(container.querySelector('.katex-display > .katex')).not.toBeNull();
    expect(container.querySelector('.katex .base, .katex .strut')).toBeNull();
  });

  it('renders Mermaid fences through the Mermaid diagram component', async () => {
    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {'```mermaid\nflowchart LR\nA --> B\n```'}
      </JinxiuMarkdown>,
    ));
    await act(async () => {
      await vi.dynamicImportSettled();
      await Promise.resolve();
    });

    expect(mermaidMocks.render).toHaveBeenCalledWith(
      expect.stringMatching(/^mmd-mermaid-/),
      'flowchart LR\nA --> B',
    );
    expect(container.querySelector('.mmd-mermaid-diagram svg')?.textContent).toContain('Mermaid diagram');
  });

  it('renders a relative HTML page embed as an interactive sandboxed frame', async () => {
    tauriMocks.invoke.mockImplementation(async (command) => {
      if (command === 'prepare_markdown_html_embed') {
        return {
          url: 'http://127.0.0.1:43127/demos/counter.html',
          ownerId: 71,
        };
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {'<iframe src="demos/counter.html" title="Counter demo" srcdoc="unsafe" onload="window.bad = true" sandbox="allow-top-navigation" allow="camera" width="999999" height="999999"></iframe>'}
      </JinxiuMarkdown>,
    ));
    await act(async () => {
      await vi.waitFor(() => {
        expect(tauriMocks.invoke).toHaveBeenCalledWith('prepare_markdown_html_embed', {
          htmlSrc: 'demos/counter.html',
          markdownPath: '/workspace/guide.md',
          workspaceRoot: '/workspace',
        });
      });
    });

    const frame = container.querySelector('iframe');
    expect(frame?.getAttribute('src')).toBe('http://127.0.0.1:43127/demos/counter.html');
    expect(frame?.getAttribute('title')).toBe('Counter demo');
    expect(frame?.getAttribute('sandbox')).toBe('allow-scripts allow-same-origin allow-forms');
    expect(frame?.hasAttribute('srcdoc')).toBe(false);
    expect(frame?.hasAttribute('onload')).toBe(false);
    expect(frame?.hasAttribute('allow')).toBe(false);
    expect(frame?.hasAttribute('width')).toBe(false);
    expect(frame?.hasAttribute('height')).toBe(false);
  });

  it('renders an mmd:embed Markdown link as an interactive sandboxed frame', async () => {
    tauriMocks.invoke.mockImplementation(async (command) => {
      if (command === 'prepare_markdown_html_embed') {
        return {
          url: 'http://127.0.0.1:43127/html/cc-switch-flow.html',
          ownerId: 73,
        };
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {'**<u>[cc-switch-flow.html](html/cc-switch-flow.html "mmd:embed")</u>**'}
      </JinxiuMarkdown>,
    ));
    await act(async () => {
      await vi.waitFor(() => {
        expect(tauriMocks.invoke).toHaveBeenCalledWith('prepare_markdown_html_embed', {
          htmlSrc: 'html/cc-switch-flow.html',
          markdownPath: '/workspace/guide.md',
          workspaceRoot: '/workspace',
        });
      });
    });

    const frame = container.querySelector('iframe');
    expect(frame?.getAttribute('src')).toBe('http://127.0.0.1:43127/html/cc-switch-flow.html');
    expect(frame?.getAttribute('title')).toBe('cc-switch-flow.html');
    expect(container.querySelector('p .mmd-html-embed-viewport > iframe')).toBe(frame);
    expect(container.querySelector('a')).toBeNull();
  });

  it('renders a parent-relative mmd:embed link that remains inside the workspace', async () => {
    tauriMocks.invoke.mockImplementation(async (command) => {
      if (command === 'prepare_markdown_html_embed') {
        return {
          url: 'http://127.0.0.1:43127/demos/counter.html',
          ownerId: 74,
        };
      }
      throw new Error(`Unexpected command: ${command}`);
    });

    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/docs/guide.md" workspaceRoot="/workspace">
        {'[Counter](../demos/counter.html "mmd:embed")'}
      </JinxiuMarkdown>,
    ));
    await act(async () => {
      await vi.waitFor(() => {
        expect(tauriMocks.invoke).toHaveBeenCalledWith('prepare_markdown_html_embed', {
          htmlSrc: '../demos/counter.html',
          markdownPath: '/workspace/docs/guide.md',
          workspaceRoot: '/workspace',
        });
      });
    });

    expect(container.querySelector('iframe')).not.toBeNull();
    expect(container.querySelector('a')).toBeNull();
  });

  it('keeps unmarked and unsafe mmd:embed links as ordinary links', async () => {
    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {[
          '[Plain HTML](demos/counter.html)',
          '[Remote HTML](https://example.com/counter.html "mmd:embed")',
          '[Outside HTML](../../counter.html "mmd:embed")',
        ].join('\n\n')}
      </JinxiuMarkdown>,
    ));

    expect([...container.querySelectorAll('a')].map((link) => link.getAttribute('href'))).toEqual([
      'demos/counter.html',
      'https://example.com/counter.html',
      '../../counter.html',
    ]);
    expect(container.querySelector('iframe')).toBeNull();
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it('does not grant popup, modal, or download privileges to Markdown HTML embeds', async () => {
    tauriMocks.invoke.mockResolvedValue({
      url: 'http://127.0.0.1:43127/demos/counter.html',
      ownerId: 72,
    });

    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {'<iframe src="demos/counter.html"></iframe>'}
      </JinxiuMarkdown>,
    ));
    await act(async () => {
      await vi.waitFor(() => expect(container.querySelector('iframe')).not.toBeNull());
    });

    const sandbox = container.querySelector('iframe')?.getAttribute('sandbox') ?? '';
    expect(sandbox).not.toContain('allow-popups');
    expect(sandbox).not.toContain('allow-modals');
    expect(sandbox).not.toContain('allow-downloads');
  });

  it('keeps arbitrary raw HTML and remote frames inert', async () => {
    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {'<button onclick="window.bad = true">Run</button>\n\n<iframe src="https://example.com/app.html"></iframe>'}
      </JinxiuMarkdown>,
    ));

    expect(container.querySelector('button, iframe')).toBeNull();
    expect(container.textContent).toContain('<button onclick="window.bad = true">Run</button>');
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it('keeps unsupported local HTML path syntax inert', async () => {
    const source = [
      '<iframe src="../outside.html"></iframe>',
      '<iframe src="demo.html?mode=preview"></iframe>',
      '<iframe src="demos\\counter.html"></iframe>',
      '<iframe src="demos%2fencoded.html"></iframe>',
    ].join('\n\n');

    await act(async () => root.render(
      <JinxiuMarkdown currentFilePath="/workspace/guide.md" workspaceRoot="/workspace">
        {source}
      </JinxiuMarkdown>,
    ));

    expect(container.querySelector('iframe')).toBeNull();
    expect(container.textContent).toContain('../outside.html');
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });

  it('keeps an unauthorized HTML embed unavailable instead of permanently loading', async () => {
    await act(async () => root.render(
      <JinxiuMarkdown
        currentFilePath="/workspace/guide.md"
        localAssetsEnabled={false}
        workspaceRoot="/workspace"
      >
        {'<iframe src="demos/counter.html"></iframe>'}
      </JinxiuMarkdown>,
    ));

    const status = container.querySelector('output');
    expect(status?.getAttribute('aria-busy')).toBe('false');
    expect(status?.textContent).toBe('HTML preview is temporarily unavailable.');
    expect(tauriMocks.invoke).not.toHaveBeenCalled();
  });
});
