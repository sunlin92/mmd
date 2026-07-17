import supportedPrismLanguages from 'react-syntax-highlighter/dist/esm/languages/prism/supported-languages';

const LANG_ALIASES: Record<string, string> = {
  ts: 'typescript',
  js: 'javascript',
  py: 'python',
  sh: 'bash',
  shell: 'bash',
  yml: 'yaml',
  md: 'markdown',
  rs: 'rust',
  rb: 'ruby',
  kt: 'kotlin',
};

const PRISM_LANG_SET = new Set(supportedPrismLanguages as unknown as string[]);

export function normalizeFenceLanguage(raw: string): string {
  const lower = (raw || 'text').toLowerCase().trim();
  return LANG_ALIASES[lower] || lower || 'text';
}

export function classNameToString(className: unknown): string {
  if (className == null) return '';
  if (Array.isArray(className)) return className.map(String).filter(Boolean).join(' ');
  return String(className);
}

export function parseFenceLangTokenFromClasses(classStr: string): string {
  for (const part of classStr.trim().split(/\s+/).filter(Boolean)) {
    if (/^language-/i.test(part)) return part.slice('language-'.length).trim();
  }
  return '';
}

export function effectivePrismLanguage(normalized: string): string {
  if (normalized === 'text') return 'text';
  return PRISM_LANG_SET.has(normalized) ? normalized : 'text';
}
