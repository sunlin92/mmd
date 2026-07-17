import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import JinxiuMarkdown from '../JinxiuMarkdown';

describe('fenced code block rendering', () => {
  it('renders one classified code surface with one line number per source line', () => {
    const html = renderToStaticMarkup(
      <JinxiuMarkdown currentFilePath={null} workspaceRoot={null}>{'```bash\nnode --version\nnpm --version\n```'}</JinxiuMarkdown>,
    );

    expect(html).toContain('class="jinxiu-code-copy-wrap markdown-code-block-copy-wrap jinxiu-code-surface"');
    expect(html.match(/<pre\b/g)).toHaveLength(1);
    expect(html).toContain('<pre class="jinxiu-code-block-pre"');
    expect(html.match(/react-syntax-highlighter-line-number/g)).toHaveLength(2);
  });
});
