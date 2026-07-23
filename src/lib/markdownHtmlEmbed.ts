import type { Element, Properties, Root } from 'hast';

interface RawHtmlNode {
  type: 'raw';
  value: string;
}

interface ParentNode {
  children: Array<ChildNode>;
}

type ChildNode = (Root['children'][number] | RawHtmlNode) & Partial<ParentNode>;

const HTML_FILE_EXTENSION_RE = /\.(?:html?|xhtml)$/i;

interface MarkdownHtmlEmbedContext {
  currentFilePath: string | null;
  workspaceRoot: string | null;
}

function trimPathEnd(path: string): string {
  const normalized = path.replace(/\\/g, '/');
  if (normalized === '/' || /^[a-z]:\/$/i.test(normalized)) return normalized;
  return normalized.replace(/\/+$/, '');
}

function parentPathStaysInsideWorkspace(
  decodedSource: string,
  context: MarkdownHtmlEmbedContext | undefined,
): boolean {
  if (!decodedSource.split('/').includes('..')) return true;
  if (!context?.currentFilePath || !context.workspaceRoot) return false;

  const workspaceRoot = trimPathEnd(context.workspaceRoot);
  const currentFilePath = trimPathEnd(context.currentFilePath);
  if (!workspaceRoot || !currentFilePath) return false;
  const caseInsensitive = /^[a-z]:\//i.test(workspaceRoot) || context.workspaceRoot.includes('\\');
  const comparableRoot = caseInsensitive ? workspaceRoot.toLowerCase() : workspaceRoot;
  const comparableFile = caseInsensitive ? currentFilePath.toLowerCase() : currentFilePath;
  const rootEndsWithSeparator = comparableRoot.endsWith('/');
  const rootPrefix = rootEndsWithSeparator ? comparableRoot : `${comparableRoot}/`;
  if (!comparableFile.startsWith(rootPrefix)) return false;

  const relativeFilePath = rootEndsWithSeparator
    ? currentFilePath.slice(workspaceRoot.length)
    : currentFilePath.slice(workspaceRoot.length + 1);
  const resolvedSegments = relativeFilePath.split('/').filter(Boolean).slice(0, -1);
  for (const component of decodedSource.split('/')) {
    if (!component || component === '.') continue;
    if (component === '..') {
      if (resolvedSegments.length === 0) return false;
      resolvedSegments.pop();
    } else {
      resolvedSegments.push(component);
    }
  }
  return true;
}

export function isLocalMarkdownHtmlEmbedSource(
  src: string,
  context?: MarkdownHtmlEmbedContext,
): boolean {
  const source = src.trim();
  if (!source || source.includes('?') || source.includes('#') || source.includes('\\')) return false;
  if (/%(?:2f|5c)/i.test(source)) return false;

  let path: string;
  try {
    path = decodeURIComponent(source);
  } catch {
    return false;
  }
  if (source.split('/').some((component) => (
    component !== '..' && decodeURIComponent(component) === '..'
  ))) return false;
  if (path.includes('?') || path.includes('#') || path.includes('\\')) return false;
  if (path.startsWith('/') || path.startsWith('~') || /^[a-z][a-z\d+.-]*:/i.test(path)) return false;
  if (!parentPathStaysInsideWorkspace(path, context)) return false;
  return HTML_FILE_EXTENSION_RE.test(path);
}

function iframeProperties(rawHtml: string): Properties | null {
  if (typeof DOMParser === 'undefined') return null;
  const document = new DOMParser().parseFromString(rawHtml.trim(), 'text/html');
  const meaningfulNodes = [...document.body.childNodes].filter((node) => (
    node.nodeType !== Node.TEXT_NODE || node.textContent?.trim()
  ));
  if (meaningfulNodes.length !== 1) return null;
  const frame = meaningfulNodes[0];
  if (!(frame instanceof HTMLElement) || frame.tagName !== 'IFRAME') return null;

  const src = frame.getAttribute('src')?.trim() ?? '';
  if (!isLocalMarkdownHtmlEmbedSource(src)) return null;

  const properties: Properties = { src };
  for (const attribute of ['title'] as const) {
    const value = frame.getAttribute(attribute)?.trim();
    if (value) properties[attribute] = value;
  }
  return properties;
}

function transformHtmlEmbeds(parent: ParentNode): void {
  parent.children = parent.children.map((child) => {
    if (child.type === 'raw') {
      const properties = iframeProperties(child.value);
      if (!properties) return child;
      return {
        type: 'element',
        tagName: 'iframe',
        properties,
        children: [],
      } satisfies Element;
    }
    if (Array.isArray(child.children)) transformHtmlEmbeds(child as ParentNode);
    return child;
  });
}

export function rehypeMarkdownHtmlEmbeds() {
  return (tree: Root) => transformHtmlEmbeds(tree as ParentNode);
}
