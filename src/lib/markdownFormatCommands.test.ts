import { describe, expect, it } from 'vitest';
import {
  applyMarkdownFormatCommand,
  MARKDOWN_FORMAT_COMMANDS,
} from './markdownFormatCommands';

describe('Markdown format commands', () => {
  it('offers the expected headings, common blocks, and alert variants', () => {
    expect(MARKDOWN_FORMAT_COMMANDS.map((command) => command.id)).toEqual([
      'h1',
      'h2',
      'h3',
      'bold',
      'italic',
      'strikethrough',
      'inline-code',
      'link',
      'blockquote',
      'bullet-list',
      'ordered-list',
      'task-list',
      'code-block',
      'alert-tip',
      'alert-info',
      'alert-warning',
      'alert-error',
    ]);
  });

  it('inserts an empty inline template and places the caret inside it', () => {
    expect(applyMarkdownFormatCommand('before after', { from: 7, to: 7 }, 'bold')).toEqual({
      from: 7,
      insert: '****',
      selection: { anchor: 9, head: 9 },
      to: 7,
    });
  });

  it('wraps selected inline text and keeps the wrapped text selected', () => {
    expect(applyMarkdownFormatCommand('before alpha after', { from: 7, to: 12 }, 'bold')).toEqual({
      from: 7,
      insert: '**alpha**',
      selection: { anchor: 9, head: 14 },
      to: 12,
    });
  });

  it('places the caret in the link destination after wrapping selected text', () => {
    expect(applyMarkdownFormatCommand('alpha', { from: 0, to: 5 }, 'link')).toEqual({
      from: 0,
      insert: '[alpha]()',
      selection: { anchor: 8, head: 8 },
      to: 5,
    });
  });

  it('prefixes every selected line for unordered and ordered lists', () => {
    expect(applyMarkdownFormatCommand('alpha\nbeta', { from: 0, to: 10 }, 'bullet-list').insert)
      .toBe('- alpha\n- beta');
    expect(applyMarkdownFormatCommand('alpha\nbeta', { from: 0, to: 10 }, 'ordered-list').insert)
      .toBe('1. alpha\n2. beta');
  });

  it.each([
    ['alert-tip', '> [!TIP]\n> selected'],
    ['alert-info', '> [!NOTE]\n> selected'],
    ['alert-warning', '> [!WARNING]\n> selected'],
    ['alert-error', '> [!CAUTION]\n> selected'],
  ] as const)('wraps selected text with the %s alert syntax', (command, expected) => {
    expect(applyMarkdownFormatCommand('selected', { from: 0, to: 8 }, command).insert)
      .toBe(expected);
  });

  it('inserts empty block templates with useful caret positions', () => {
    expect(applyMarkdownFormatCommand('', { from: 0, to: 0 }, 'h2')).toMatchObject({
      insert: '## ',
      selection: { anchor: 3, head: 3 },
    });
    expect(applyMarkdownFormatCommand('', { from: 0, to: 0 }, 'code-block')).toMatchObject({
      insert: '```\n\n```',
      selection: { anchor: 4, head: 4 },
    });
    expect(applyMarkdownFormatCommand('', { from: 0, to: 0 }, 'alert-tip')).toMatchObject({
      insert: '> [!TIP]\n> ',
      selection: { anchor: 11, head: 11 },
    });
  });

  it('isolates an empty block template when the caret is inside a paragraph', () => {
    expect(applyMarkdownFormatCommand('beforeafter', { from: 6, to: 6 }, 'alert-tip')).toEqual({
      from: 6,
      insert: '\n> [!TIP]\n> \n',
      selection: { anchor: 18, head: 18 },
      to: 6,
    });
  });
});
