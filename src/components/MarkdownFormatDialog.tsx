import {
  Bold,
  CircleX,
  Code2,
  Heading1,
  Heading2,
  Heading3,
  Info,
  Italic,
  Lightbulb,
  Link2,
  List,
  ListChecks,
  ListOrdered,
  Quote,
  Search,
  SquareCode,
  Strikethrough,
  TriangleAlert,
  type LucideIcon,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  MARKDOWN_FORMAT_COMMANDS,
  type MarkdownFormatCommand,
  type MarkdownFormatCommandId,
} from '../lib/markdownFormatCommands';
import { useI18n } from '../lib/i18n';

const COMMAND_ZH: Record<MarkdownFormatCommandId, string> = {
  h1: '一级标题', h2: '二级标题', h3: '三级标题', bold: '粗体', italic: '斜体',
  strikethrough: '删除线', 'inline-code': '行内代码', link: '链接', blockquote: '引用',
  'bullet-list': '无序列表', 'ordered-list': '有序列表', 'task-list': '任务列表',
  'code-block': '代码块', 'alert-tip': '提示块', 'alert-info': '信息块',
  'alert-warning': '警告块', 'alert-error': '错误块',
};

const CATEGORY_ZH: Record<string, string> = { Text: '文本', Blocks: '块', Alerts: '提示' };

interface MarkdownFormatDialogProps {
  onCancel: () => void;
  onSelect: (command: MarkdownFormatCommandId) => void;
}

const COMMAND_ICONS: Record<MarkdownFormatCommandId, LucideIcon> = {
  h1: Heading1,
  h2: Heading2,
  h3: Heading3,
  bold: Bold,
  italic: Italic,
  strikethrough: Strikethrough,
  'inline-code': Code2,
  link: Link2,
  blockquote: Quote,
  'bullet-list': List,
  'ordered-list': ListOrdered,
  'task-list': ListChecks,
  'code-block': SquareCode,
  'alert-tip': Lightbulb,
  'alert-info': Info,
  'alert-warning': TriangleAlert,
  'alert-error': CircleX,
};

function commandMatches(command: Pick<MarkdownFormatCommand, 'category' | 'keywords' | 'label' | 'syntax'> | { category: string; keywords: string; label: string; syntax: string }, query: string): boolean {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return true;
  return `${command.label} ${command.category} ${command.keywords} ${command.syntax}`
    .toLowerCase()
    .includes(normalized);
}

export function MarkdownFormatDialog({ onCancel, onSelect }: MarkdownFormatDialogProps) {
  const { locale, t } = useI18n();
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const commands = useMemo(
    () => MARKDOWN_FORMAT_COMMANDS
      .map((command) => locale === 'zh-CN' ? { ...command, label: COMMAND_ZH[command.id], category: CATEGORY_ZH[command.category] ?? command.category } : command)
      .filter((command) => commandMatches(command, query)),
    [locale, query],
  );

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  const chooseActiveCommand = () => {
    const command = commands[activeIndex];
    if (command) onSelect(command.id);
  };

  return (
    <div
      className="markdown-format-dialog-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <dialog
        open
        className="markdown-format-dialog"
        aria-modal="true"
        aria-labelledby="markdown-format-dialog-title"
        onKeyDown={(event) => {
          if (event.key !== 'Escape') return;
          event.preventDefault();
          event.stopPropagation();
          onCancel();
        }}
      >
        <div className="markdown-format-dialog-header">
          <h2 id="markdown-format-dialog-title">{t('format')}</h2>
          <label className="markdown-format-search">
            <Search size={15} aria-hidden="true" />
            <input
              ref={inputRef}
              role="combobox"
              aria-autocomplete="list"
              aria-controls="markdown-format-command-list"
              aria-expanded="true"
              aria-haspopup="menu"
              aria-label={t('searchFormatCommands')}
              aria-activedescendant={commands[activeIndex] ? `markdown-format-${commands[activeIndex].id}` : undefined}
              placeholder={t('searchFormats')}
              spellCheck={false}
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === 'ArrowDown') {
                  event.preventDefault();
                  setActiveIndex((index) => commands.length ? (index + 1) % commands.length : 0);
                } else if (event.key === 'ArrowUp') {
                  event.preventDefault();
                  setActiveIndex((index) => commands.length ? (index - 1 + commands.length) % commands.length : 0);
                } else if (event.key === 'Enter') {
                  event.preventDefault();
                  chooseActiveCommand();
                }
              }}
            />
          </label>
        </div>
        <div
          id="markdown-format-command-list"
          className="markdown-format-command-list"
          role="menu"
          aria-label={t('formatCommands')}
        >
          {commands.map((command, index) => {
            const Icon = COMMAND_ICONS[command.id];
            return (
              <button
                key={command.id}
                id={`markdown-format-${command.id}`}
                type="button"
                role="menuitem"
                className={index === activeIndex ? 'markdown-format-command active' : 'markdown-format-command'}
                data-command-id={command.id}
                onClick={() => onSelect(command.id)}
                onFocus={() => setActiveIndex(index)}
                onMouseMove={() => setActiveIndex(index)}
              >
                <Icon className={`markdown-format-command-icon ${command.id}`} size={16} aria-hidden="true" />
                <span className="markdown-format-command-copy">
                  <strong>{command.label}</strong>
                  <small>{command.category}</small>
                </span>
                <code>{command.syntax}</code>
              </button>
            );
          })}
          {commands.length === 0 && <p className="markdown-format-empty">{t('noMatchingFormats')}</p>}
        </div>
      </dialog>
    </div>
  );
}
