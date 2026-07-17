// @vitest-environment jsdom

import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { resetImagePreviewCache } from '../lib/workspacePreviewSource';
import JinxiuMarkdown from './JinxiuMarkdown';

const tauriMocks = vi.hoisted(() => ({
  convertFileSrc: vi.fn<(path: string) => string>((path) => `asset://localhost/${encodeURIComponent(path)}`),
  invoke: vi.fn<(command: string, payload?: unknown) => Promise<string>>(),
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
});
