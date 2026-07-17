import type { WorkspaceFileEntry } from '../types';

type MarkdownMediaAsset = Pick<WorkspaceFileEntry, 'kind' | 'name' | 'relative_path'>;
type MarkdownMediaDocument = Pick<WorkspaceFileEntry, 'relative_path'>;

export interface MarkdownMediaInsertion {
  clientX: number;
  clientY: number;
  documentEpoch: number;
  documentId: string;
  markdown: string;
  requestId: number;
}

function splitWorkspaceRelativePath(path: string): string[] | null {
  const normalized = path.replace(/\\/g, '/');
  if (!normalized || normalized.startsWith('/')) return null;
  const segments = normalized.split('/');
  if (segments.some((segment) => !segment || segment === '.' || segment === '..')) return null;
  return segments;
}

function encodeMarkdownDestinationSegment(segment: string): string {
  return encodeURIComponent(segment).replace(/[!'()*]/g, (character) => (
    `%${character.charCodeAt(0).toString(16).toUpperCase()}`
  ));
}

function encodeMarkdownDestination(segments: string[]): string {
  return segments.map((segment) => (
    segment === '..' ? segment : encodeMarkdownDestinationSegment(segment)
  )).join('/');
}

function escapeMarkdownLabel(label: string): string {
  return label.replace(/[\\[\]]/g, '\\$&');
}

function relativeAssetPath(
  document: MarkdownMediaDocument,
  asset: MarkdownMediaAsset,
): string | null {
  const documentSegments = splitWorkspaceRelativePath(document.relative_path);
  const assetSegments = splitWorkspaceRelativePath(asset.relative_path);
  if (!documentSegments || !assetSegments) return null;
  const documentDirectory = documentSegments.slice(0, -1);
  let commonLength = 0;
  while (
    commonLength < documentDirectory.length
    && commonLength < assetSegments.length
    && documentDirectory[commonLength] === assetSegments[commonLength]
  ) {
    commonLength += 1;
  }
  return encodeMarkdownDestination([
    ...Array.from({ length: documentDirectory.length - commonLength }, () => '..'),
    ...assetSegments.slice(commonLength),
  ]);
}

export function createMarkdownMediaReference(
  asset: MarkdownMediaAsset,
  document: MarkdownMediaDocument,
): string | null {
  if (asset.kind !== 'image' && asset.kind !== 'audio') return null;
  const destination = relativeAssetPath(document, asset);
  if (!destination) return null;
  const label = escapeMarkdownLabel(asset.name);
  return asset.kind === 'image'
    ? `![${label}](${destination})`
    : `[${label}](${destination})`;
}
