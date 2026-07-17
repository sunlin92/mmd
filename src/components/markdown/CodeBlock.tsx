import { useCallback, useMemo, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import oneLight from 'react-syntax-highlighter/dist/esm/styles/prism/one-light';
import oneDark from 'react-syntax-highlighter/dist/esm/styles/prism/one-dark';
import { effectivePrismLanguage, normalizeFenceLanguage } from './markdownLanguage';
import { useI18n } from '../../lib/i18n';

type PrismStyleSheet = typeof oneLight;

function stripThemePreLayout(base: PrismStyleSheet): PrismStyleSheet {
  return { ...base, 'pre[class*="language-"]': {} };
}

function flatCodeSurface(base: PrismStyleSheet): PrismStyleSheet {
  const codeRule = base['code[class*="language-"]'];
  return codeRule ? { ...base, 'code[class*="language-"]': { ...codeRule, backgroundColor: 'transparent' } } : base;
}

export function CodeBlock({ code, language }: { code: string; language: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const dark = typeof document !== 'undefined' && document.documentElement.classList.contains('dark');
  const body = code.replace(/\n$/, '');
  const lang = effectivePrismLanguage(normalizeFenceLanguage(language));
  const prismStyle = useMemo(() => flatCodeSurface(stripThemePreLayout(dark ? oneDark : oneLight)), [dark]);

  const copy = useCallback(async () => {
    if (!body.trim()) return;
    await navigator.clipboard.writeText(body);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  }, [body]);

  return (
    <div className="jinxiu-code-copy-wrap markdown-code-block-copy-wrap jinxiu-code-surface">
      <button className="code-copy-button" type="button" onClick={copy} title={copied ? t('copied') : t('copyCode')} aria-label={copied ? t('copied') : t('copyCode')}>
        {copied ? <Check size={15} /> : <Copy size={15} />}
      </button>
      <SyntaxHighlighter
        className="jinxiu-code-block-pre"
        language={lang}
        style={prismStyle}
        PreTag="pre"
        showLineNumbers
        wrapLines
        customStyle={{ margin: 0, borderRadius: 0, fontSize: '14px', lineHeight: 1.62, padding: '14px 0 18px', backgroundColor: dark ? '#0d1117' : '#fff' }}
        lineNumberStyle={{ minWidth: '3rem', paddingRight: '0.9rem', color: dark ? '#6e95c9' : '#1f82a6', userSelect: 'none' }}
        lineProps={{ className: 'jinxiu-code-line' }}
        codeTagProps={{ className: 'jinxiu-fenced-code-inner' }}
      >
        {body}
      </SyntaxHighlighter>
    </div>
  );
}
