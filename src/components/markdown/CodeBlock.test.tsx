// @vitest-environment jsdom

import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const highlighterMocks = vi.hoisted(() => ({
  loaded: vi.fn<() => void>(),
}));

const clipboardMocks = vi.hoisted(() => ({
  writeText: vi.fn<(text: string) => Promise<void>>(),
}));

vi.mock('react-syntax-highlighter', async () => {
  highlighterMocks.loaded();
  return vi.importActual('react-syntax-highlighter');
});

describe('fenced code block rendering', () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    highlighterMocks.loaded.mockClear();
    clipboardMocks.writeText.mockReset();
    clipboardMocks.writeText.mockResolvedValue();
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: clipboardMocks.writeText },
    });
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('loads syntax highlighting only after the first non-Mermaid fence', async () => {
    const { default: JinxiuMarkdown } = await import('../JinxiuMarkdown');

    act(() => root.render(
      <JinxiuMarkdown currentFilePath={null} workspaceRoot={null}>Plain Markdown</JinxiuMarkdown>,
    ));
    expect(highlighterMocks.loaded).not.toHaveBeenCalled();

    act(() => root.render(
      <JinxiuMarkdown currentFilePath={null} workspaceRoot={null}>{'```bash\nnode --version\nnpm --version\n```'}</JinxiuMarkdown>,
    ));

    const fallback = container.querySelector('.jinxiu-code-surface pre[aria-busy="true"]');
    expect(fallback?.querySelector('code')?.textContent).toBe('node --version\nnpm --version');

    await act(async () => {
      await vi.dynamicImportSettled();
      await Promise.resolve();
    });

    expect(highlighterMocks.loaded).toHaveBeenCalledTimes(1);
    expect(container.querySelector('.jinxiu-code-surface [aria-busy="true"]')).toBeNull();
    expect(container.querySelector('pre.jinxiu-code-block-pre code.jinxiu-fenced-code-inner')).not.toBeNull();
    expect(container.querySelectorAll('.react-syntax-highlighter-line-number')).toHaveLength(2);
    const copyButton = container.querySelector<HTMLButtonElement>('.code-copy-button');
    expect(copyButton?.getAttribute('aria-label')).toBe('Copy code');
    await act(async () => copyButton?.click());
    expect(clipboardMocks.writeText).toHaveBeenCalledWith('node --version\nnpm --version');
    expect(container.querySelector('pre')?.style.backgroundColor).toBe('rgb(255, 255, 255)');
  }, 15_000);
});
