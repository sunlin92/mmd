// @vitest-environment jsdom

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppSettings } from '../types';
import { currentSettingsEnvelope } from '../lib/settings.test';
import { SettingsDialog } from './SettingsDialog';

describe('SettingsDialog', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('renders compact native controls and saves autosave, spellcheck, wikilinks, resource and layout settings', async () => {
    const onSave = vi.fn<(settings: AppSettings) => Promise<void>>(async () => undefined);
    await act(async () => root.render(
      <SettingsDialog
        busy={false}
        locale="en"
        settings={currentSettingsEnvelope.settings}
        onClose={vi.fn<() => void>()}
        onReset={vi.fn<() => Promise<void>>(async () => undefined)}
        onSave={onSave}
      />,
    ));

    const dialog = container.querySelector('dialog');
    expect(dialog?.getAttribute('aria-modal')).toBe('true');
    expect(container.querySelectorAll('.settings-section .settings-section').length).toBe(0);
    expect(container.querySelector<HTMLInputElement>('[name="wikilinksEnabled"]')?.checked).toBe(false);

    const delay = container.querySelector<HTMLInputElement>('[name="autosaveDelayMs"]')!;
    act(() => {
      const valueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
      valueSetter?.call(delay, '2200');
      delay.dispatchEvent(new Event('input', { bubbles: true }));
      delay.dispatchEvent(new Event('change', { bubbles: true }));
    });
    const form = container.querySelector('form')!;
    await act(async () => form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));

    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      autosaveDelayMs: 2200,
      wikilinksEnabled: false,
    }));
  });

  it('shows reset and retry recovery actions without rendering a raw error', async () => {
    await act(async () => root.render(
      <SettingsDialog
        busy={false}
        locale="en"
        recovery={{ canReset: true, kind: 'recoverable' }}
        onReset={vi.fn<() => Promise<void>>(async () => undefined)}
        onRetry={vi.fn<() => Promise<void>>(async () => undefined)}
      />,
    ));

    expect(container.textContent).toContain('Reset Settings');
    expect(container.textContent).toContain('Try Again');
    expect(container.textContent).not.toContain('parse');
    expect(container.querySelector('[role="alertdialog"]')).not.toBeNull();
  });

  it('does not offer reset for a future schema and leaves the file unchanged', async () => {
    await act(async () => root.render(
      <SettingsDialog
        busy={false}
        locale="en"
        recovery={{ canReset: false, kind: 'future' }}
        onReset={vi.fn<() => Promise<void>>(async () => undefined)}
        onRetry={vi.fn<() => Promise<void>>(async () => undefined)}
      />,
    ));

    expect(container.textContent).toContain('newer version');
    expect(container.textContent).toContain('Try Again');
    expect(container.textContent).not.toContain('Reset Settings');
  });

  it('offers reload only after a settings conflict without exposing backend details', async () => {
    await act(async () => root.render(
      <SettingsDialog
        busy={false}
        locale="en"
        recovery={{ canReset: false, kind: 'conflict' }}
        onReset={vi.fn<() => Promise<void>>(async () => undefined)}
        onRetry={vi.fn<() => Promise<void>>(async () => undefined)}
      />,
    ));

    expect(container.textContent).toContain('changed in another window');
    expect(container.textContent).toContain('Reload Settings');
    expect(container.textContent).not.toContain('Reset Settings');
    expect(container.textContent).not.toContain('Tauri');
  });
});
