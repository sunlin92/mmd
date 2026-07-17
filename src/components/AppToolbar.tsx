import { Check, CircleAlert, FileText, LoaderCircle } from 'lucide-react';
import { displayName } from '../lib/documentNames';
import { useI18n } from '../lib/i18n';

interface AppToolbarProps {
  activePath: string | null;
  busy: boolean;
  dirty: boolean;
}

export function AppToolbar({ activePath, busy, dirty }: AppToolbarProps) {
  const { t } = useI18n();
  const status = busy
    ? { className: 'is-working', icon: <LoaderCircle size={13} />, label: t('working') }
    : dirty
      ? { className: 'is-edited', icon: <CircleAlert size={13} />, label: t('edited') }
      : { className: 'is-saved', icon: <Check size={13} />, label: t('saved') };

  return (
    <header className="toolbar" data-tauri-drag-region>
      <div className="brand">
        <span className="brand-mark" aria-hidden="true"><FileText size={15} /></span>
        <div className="brand-copy">
          <strong>MMD</strong>
          <small>Markdown</small>
        </div>
      </div>
      <div
        className="toolbar-document"
        aria-label={t('currentDocument')}
        title={activePath ?? undefined}
      >
        {displayName(activePath)}
      </div>
      <div className={`document-status ${status.className}`} aria-live="polite">
        {status.icon}
        <span>{status.label}</span>
      </div>
    </header>
  );
}
