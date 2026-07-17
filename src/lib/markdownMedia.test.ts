import { describe, expect, it } from 'vitest';
import { createMarkdownMediaReference } from './markdownMedia';

const markdownDocument = { relative_path: 'guide.md' };

describe('Markdown media references', () => {
  it('creates an image reference from a workspace-relative asset path', () => {
    expect(createMarkdownMediaReference({
      kind: 'image',
      name: 'cover image.png',
      relative_path: 'assets/cover image.png',
    }, markdownDocument)).toBe('![cover image.png](assets/cover%20image.png)');
  });

  it('escapes Markdown destination punctuation in an image path', () => {
    expect(createMarkdownMediaReference({
      kind: 'image',
      name: 'cover (final).png',
      relative_path: 'assets/cover (final).png',
    }, markdownDocument)).toBe('![cover (final).png](assets/cover%20%28final%29.png)');
  });

  it('creates an unambiguous reference relative to a nested Markdown document', () => {
    expect(createMarkdownMediaReference({
      kind: 'image',
      name: 'cover.png',
      relative_path: 'assets/cover.png',
    }, {
      relative_path: 'docs/guide.md',
    })).toBe('![cover.png](../assets/cover.png)');
  });

  it('creates a portable Markdown link for an audio asset', () => {
    expect(createMarkdownMediaReference({
      kind: 'audio',
      name: 'opening theme.mp3',
      relative_path: 'audio/opening theme.mp3',
    }, markdownDocument)).toBe('[opening theme.mp3](audio/opening%20theme.mp3)');
  });

  it('escapes Markdown destination punctuation in an audio path', () => {
    expect(createMarkdownMediaReference({
      kind: 'audio',
      name: 'intro (v2).mp3',
      relative_path: 'audio/intro (v2).mp3',
    }, markdownDocument)).toBe('[intro (v2).mp3](audio/intro%20%28v2%29.mp3)');
  });

  it('rejects unsupported assets and unsafe workspace paths', () => {
    expect(createMarkdownMediaReference({
      kind: 'video',
      name: 'clip.mp4',
      relative_path: 'media/clip.mp4',
    }, markdownDocument)).toBeNull();
    expect(createMarkdownMediaReference({
      kind: 'image',
      name: 'private.png',
      relative_path: '../private.png',
    }, markdownDocument)).toBeNull();
  });
});
