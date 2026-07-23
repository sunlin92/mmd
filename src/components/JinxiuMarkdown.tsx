import React, { useDeferredValue, useMemo } from 'react';
import ReactMarkdown, { type Components, type ExtraProps } from 'react-markdown';
import type { Element as HastElement } from 'hast';
import type { JSX } from 'react';
import remarkBreaks from 'remark-breaks';
import remarkCjkFriendly from 'remark-cjk-friendly';
import remarkGfm from 'remark-gfm';
import remarkGithubAlerts from 'remark-github-alerts';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import '../../public/styles/typora-theme/typora-jinxiu.css';
import { mapH2ChildrenWrapHeadingSymbols } from '../lib/headingPunct';
import { preprocessMarkdown } from '../lib/markdownPreprocess';
import { isLocalMarkdownHtmlEmbedSource, rehypeMarkdownHtmlEmbeds } from '../lib/markdownHtmlEmbed';
import AdaptiveMarkdownImage from './AdaptiveMarkdownImage';
import { MarkdownHtmlFrame } from './MarkdownHtmlFrame';
import { CodeBlock } from './markdown/CodeBlock';
import { MermaidDiagram } from './markdown/MermaidDiagram';
import { classNameToString, normalizeFenceLanguage, parseFenceLangTokenFromClasses } from './markdown/markdownLanguage';
import {
  BLOCKQUOTE_TREE_HEURISTIC_RE,
  extractText,
  hastCollectPlainText,
  isFenceCodeLike,
  paragraphIsSingleUrlLine,
  paragraphLooksLikeTextDiagram,
  slugify,
} from './markdown/markdownText';

interface Props {
  children: string;
  currentFilePath: string | null;
  localAssetsEnabled?: boolean;
  workspaceRoot: string | null;
}

type MdCodeProps = JSX.IntrinsicElements['code'] & ExtraProps & { inline?: boolean };

function headingAttributes(children: React.ReactNode, node?: HastElement) {
  const text = extractText(children);
  const line = node?.position?.start.line;
  return {
    'data-heading-key': text,
    'data-heading-slug': slugify(text),
    ...(typeof line === 'number' ? { 'data-heading-line': line } : {}),
  };
}

export default function JinxiuMarkdown({ children, currentFilePath, localAssetsEnabled = true, workspaceRoot }: Props) {
  const document = useMemo(() => ({
    children,
    currentFilePath,
    localAssetsEnabled,
    workspaceRoot,
  }), [children, currentFilePath, localAssetsEnabled, workspaceRoot]);
  const deferredDocument = useDeferredValue(document);
  const source = useMemo(() => preprocessMarkdown(deferredDocument.children), [deferredDocument.children]);
  const deferredCurrentFilePath = deferredDocument.currentFilePath;
  const deferredLocalAssetsEnabled = deferredDocument.localAssetsEnabled;
  const deferredWorkspaceRoot = deferredDocument.workspaceRoot;
  const components = useMemo<Components>(() => ({
    h1: ({ children: c, node, ...props }) => <h1 {...props} {...headingAttributes(c, node)}>{c}</h1>,
    h2: ({ children: c, node, ...props }) => <h2 {...props} {...headingAttributes(c, node)}>{mapH2ChildrenWrapHeadingSymbols(c)}</h2>,
    h3: ({ children: c, node, ...props }) => <h3 {...props} {...headingAttributes(c, node)}>{c}</h3>,
    h4: ({ children: c, node, ...props }) => <h4 {...props} {...headingAttributes(c, node)}>{c}</h4>,
    h5: ({ children: c, node, ...props }) => <h5 {...props} {...headingAttributes(c, node)}>{c}</h5>,
    h6: ({ children: c, node, ...props }) => <h6 {...props} {...headingAttributes(c, node)}>{c}</h6>,
    pre: ({ children: c }) => <>{c}</>,
    p: ({ children: c, className, ...props }) => {
      const plain = extractText(c).trim();
      const cn = [paragraphIsSingleUrlLine(c) ? 'jinxiu-qa-p-url-nowrap' : '', paragraphLooksLikeTextDiagram(plain) ? 'jinxiu-text-diagram' : '', className].filter(Boolean).join(' ') || undefined;
      return <p {...props} className={cn}>{c}</p>;
    },
    a: ({ href, title, children: c, ...props }) => {
      if (
        title === 'mmd:embed'
        && typeof href === 'string'
        && isLocalMarkdownHtmlEmbedSource(href, {
          currentFilePath: deferredCurrentFilePath,
          workspaceRoot: deferredWorkspaceRoot,
        })
      ) {
        return (
          <MarkdownHtmlFrame
            currentFilePath={deferredCurrentFilePath}
            enabled={deferredLocalAssetsEnabled}
            htmlSrc={href}
            title={extractText(c).trim() || undefined}
            workspaceRoot={deferredWorkspaceRoot}
          />
        );
      }
      const external = typeof href === 'string' && (/^https?:\/\//i.test(href) || href.startsWith('//'));
      return <a href={href} title={title} {...props} {...(external ? { target: '_blank', rel: 'noopener noreferrer' } : {})}>{c}</a>;
    },
    img: ({ src, alt, title, ...props }) => <AdaptiveMarkdownImage {...props} src={src} alt={alt ?? ''} title={title} currentFilePath={deferredCurrentFilePath} localAssetsEnabled={deferredLocalAssetsEnabled} workspaceRoot={deferredWorkspaceRoot} />,
    iframe: ({ src, title }) => (
      <MarkdownHtmlFrame
        currentFilePath={deferredCurrentFilePath}
        enabled={deferredLocalAssetsEnabled}
        htmlSrc={src ?? ''}
        title={title}
        workspaceRoot={deferredWorkspaceRoot}
      />
    ),
    blockquote: ({ node, children: c, className, ...props }) => {
      const cn = [className, BLOCKQUOTE_TREE_HEURISTIC_RE.test(hastCollectPlainText(node)) ? 'jinxiu-bq-tree' : ''].filter(Boolean).join(' ') || undefined;
      return <blockquote {...props} className={cn}>{c}</blockquote>;
    },
    code: ({ className, children: c, node, inline, ...props }: MdCodeProps) => {
      const classStr = classNameToString(className);
      const body = String(c).replace(/\n$/, '');
      const hastEl = node?.type === 'element' ? (node as HastElement) : undefined;
      const isBlock = typeof inline === 'boolean' ? !inline : isFenceCodeLike(hastEl, classStr, body);
      if (!isBlock) return <code {...props} className={classStr || undefined}>{c}</code>;
      const language = normalizeFenceLanguage(parseFenceLangTokenFromClasses(classStr));
      return language === 'mermaid'
        ? <MermaidDiagram code={body} />
        : <CodeBlock code={body} language={language} />;
    },
  }), [deferredCurrentFilePath, deferredLocalAssetsEnabled, deferredWorkspaceRoot]);

  return (
    <div className="typora-jinxiu mmd-preview-content">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks, remarkCjkFriendly, remarkMath, remarkGithubAlerts] as never}
        rehypePlugins={[rehypeMarkdownHtmlEmbeds, [rehypeKatex, { throwOnError: false, strict: false }]] as never}
        components={components}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
}
