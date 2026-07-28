import { describe, expect, it } from 'vitest';
import {
  DEFAULT_THEME_PREFERENCE,
  SKIN_IDS,
  THEME_STORAGE_KEY,
  applyEffectiveTheme,
  bootstrapTheme,
  decodeThemePreference,
  resolveEffectiveTheme,
  type ThemeRoot,
  type ThemeStorage,
} from './theme';

class MemoryThemeStorage implements ThemeStorage {
  readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
}

function createRoot(): ThemeRoot & { values: Map<string, string> } {
  const values = new Map<string, string>();
  return {
    values,
    setAttribute(name, value) {
      values.set(name, value);
    },
  };
}

describe('theme preference domain', () => {
  it('accepts exactly the five canonical skin identifiers', () => {
    expect(SKIN_IDS).toEqual([
      'jinxiu-zhusha',
      'ruyao-tianqing',
      'qinghua-jilan',
      'songke-zhuying',
      'shanshui-yemo',
    ]);

    for (const selectedSkin of SKIN_IDS) {
      expect(decodeThemePreference({ version: 1, selectedSkin, followSystem: true })).toEqual({
        version: 1,
        selectedSkin,
        followSystem: true,
      });
    }
  });

  it.each([
    null,
    undefined,
    {},
    { version: 2, selectedSkin: 'jinxiu-zhusha', followSystem: false },
    { version: 1, selectedSkin: 'unknown', followSystem: false },
    { version: 1, selectedSkin: 'jinxiu-zhusha', followSystem: 'yes' },
  ])('rejects malformed and unsupported preferences: %j', (input) => {
    expect(decodeThemePreference(input)).toBeNull();
  });

  it('resolves system dark to night and restores the selected light skin', () => {
    const preference = { version: 1, selectedSkin: 'ruyao-tianqing', followSystem: true } as const;

    expect(resolveEffectiveTheme(preference, false)).toEqual({
      skin: 'ruyao-tianqing',
      appearance: 'light',
    });
    expect(resolveEffectiveTheme(preference, true)).toEqual({
      skin: 'shanshui-yemo',
      appearance: 'dark',
    });
    expect(resolveEffectiveTheme(preference, false).skin).toBe('ruyao-tianqing');
  });

  it('keeps a directly selected night skin dark regardless of OS appearance', () => {
    const preference = { version: 1, selectedSkin: 'shanshui-yemo', followSystem: false } as const;
    expect(resolveEffectiveTheme(preference, false).appearance).toBe('dark');
    expect(resolveEffectiveTheme(preference, true).appearance).toBe('dark');
  });

  it('applies both root attributes atomically through one pure boundary', () => {
    const root = createRoot();
    applyEffectiveTheme(root, { skin: 'qinghua-jilan', appearance: 'light' });
    expect(root.values).toEqual(new Map([
      ['data-skin', 'qinghua-jilan'],
      ['data-appearance', 'light'],
    ]));
  });

  it('bootstraps valid storage synchronously before React and repairs corrupt storage', () => {
    const storage = new MemoryThemeStorage();
    const root = createRoot();
    storage.values.set(THEME_STORAGE_KEY, JSON.stringify({
      version: 1,
      selectedSkin: 'songke-zhuying',
      followSystem: false,
    }));

    expect(bootstrapTheme({ root, storage, systemDark: false }).preference.selectedSkin)
      .toBe('songke-zhuying');
    expect(root.values.get('data-skin')).toBe('songke-zhuying');

    storage.values.set(THEME_STORAGE_KEY, '{not json');
    const repaired = bootstrapTheme({ root, storage, systemDark: true });
    expect(repaired.preference).toEqual(DEFAULT_THEME_PREFERENCE);
    expect(root.values.get('data-skin')).toBe('jinxiu-zhusha');
    expect(root.values.get('data-appearance')).toBe('light');
    expect(storage.values.get(THEME_STORAGE_KEY)).toBe(JSON.stringify(DEFAULT_THEME_PREFERENCE));
  });
});
