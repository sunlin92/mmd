import { describe, expect, it } from 'vitest';
import { getFileTreeContextMenuItems, type FileTreeContextTarget } from './fileTreeContextMenu';

describe('file tree context menu model', () => {
  it('shows create actions for workspace root', () => {
    const target: FileTreeContextTarget = { kind: 'root', name: 'workspace root', path: '/workspace' };
    expect(getFileTreeContextMenuItems(target).map((item) => item.action)).toEqual(['create-file', 'create-folder', 'refresh']);
  });

  it('shows create and destructive actions for folders', () => {
    const target: FileTreeContextTarget = { kind: 'folder', name: 'notes', path: '/workspace/notes' };
    expect(getFileTreeContextMenuItems(target).map((item) => item.action)).toEqual(['create-file', 'create-folder', 'rename', 'move', 'delete']);
  });

  it('shows open, rename, and delete actions for files', () => {
    const target: FileTreeContextTarget = { fileKind: 'markdown', kind: 'file', name: 'draft.md', path: '/workspace/draft.md' };
    expect(getFileTreeContextMenuItems(target).map((item) => item.action)).toEqual(['open', 'rename', 'move', 'delete']);
  });

  it('omits rename for read-only PDF and DOCX documents', () => {
    for (const fileKind of ['pdf', 'docx'] as const) {
      const target: FileTreeContextTarget = {
        fileKind,
        kind: 'file',
        name: `guide.${fileKind}`,
        path: `/workspace/guide.${fileKind}`,
      };
      expect(getFileTreeContextMenuItems(target).map((item) => item.action)).toEqual([
        'open',
        'move',
        'delete',
      ]);
    }
  });
});
