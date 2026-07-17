export type FileTreeContextTargetKind = 'file' | 'folder' | 'root';

import type { WorkspaceFileKind } from '../types';

export type FileTreeContextTarget =
  | { kind: 'root'; name: string; path: string }
  | { kind: 'folder'; name: string; path: string }
  | { fileKind: WorkspaceFileKind; kind: 'file'; name: string; path: string };

export type FileTreeContextAction = 'create-file' | 'create-folder' | 'delete' | 'move' | 'open' | 'refresh' | 'rename';

export interface FileTreeContextMenuItem {
  action: FileTreeContextAction;
  danger?: boolean;
  label: string;
  separatorBefore?: boolean;
  shortcut?: string;
}

export function canRenameFileTreeTarget(target: FileTreeContextTarget): boolean {
  return target.kind !== 'root'
    && (target.kind === 'folder' || (target.fileKind !== 'pdf' && target.fileKind !== 'docx'));
}

export function getFileTreeContextMenuItems(target: FileTreeContextTarget): FileTreeContextMenuItem[] {
  if (target.kind === 'root') {
    return [
      { action: 'create-file', label: 'New Markdown File' },
      { action: 'create-folder', label: 'New Folder' },
      { action: 'refresh', label: 'Refresh', separatorBefore: true, shortcut: '⌘R' },
    ];
  }

  if (target.kind === 'folder') {
    return [
      { action: 'create-file', label: 'New Markdown File' },
      { action: 'create-folder', label: 'New Folder' },
      { action: 'rename', label: 'Rename', separatorBefore: true, shortcut: 'Return' },
      { action: 'move', label: 'Move…' },
      { action: 'delete', danger: true, label: 'Delete', separatorBefore: true, shortcut: '⌘⌫' },
    ];
  }

  const items: FileTreeContextMenuItem[] = [
    { action: 'open', label: 'Open', shortcut: '⌘O' },
  ];
  if (canRenameFileTreeTarget(target)) {
    items.push({ action: 'rename', label: 'Rename', separatorBefore: true, shortcut: 'Return' });
  }
  items.push({ action: 'move', label: 'Move…', separatorBefore: !canRenameFileTreeTarget(target) });
  items.push({ action: 'delete', danger: true, label: 'Delete', separatorBefore: true, shortcut: '⌘⌫' });
  return items;
}
