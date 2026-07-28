export const SKIN_IDS = [
  'jinxiu-zhusha',
  'ruyao-tianqing',
  'qinghua-jilan',
  'songke-zhuying',
  'shanshui-yemo',
] as const;

export const THEME_PREFERENCE_VERSION = 1 as const;
export const THEME_PROTOCOL_VERSION = 1 as const;
export const THEME_STORAGE_KEY = 'mmd-theme-preference';
export const THEME_SNAPSHOT_EVENT = 'mmd-theme-preference';

export type SkinId = (typeof SKIN_IDS)[number];
export type ThemeAppearance = 'light' | 'dark';

export interface ThemePreference {
  readonly version: typeof THEME_PREFERENCE_VERSION;
  readonly selectedSkin: SkinId;
  readonly followSystem: boolean;
}

export interface EffectiveTheme {
  readonly skin: SkinId;
  readonly appearance: ThemeAppearance;
}

export interface ThemeSnapshotEnvelope {
  readonly protocolVersion: typeof THEME_PROTOCOL_VERSION;
  readonly revision: number;
  readonly preference: ThemePreference;
}

export interface ThemeRoot {
  setAttribute(name: string, value: string): void;
}

export interface ThemeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export const DEFAULT_THEME_PREFERENCE: ThemePreference = Object.freeze({
  version: THEME_PREFERENCE_VERSION,
  selectedSkin: 'jinxiu-zhusha',
  followSystem: false,
});

export function isSkinId(value: unknown): value is SkinId {
  return typeof value === 'string' && (SKIN_IDS as readonly string[]).includes(value);
}

export function decodeThemePreference(value: unknown): ThemePreference | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const candidate = value as Record<string, unknown>;
  if (
    candidate.version !== THEME_PREFERENCE_VERSION
    || !isSkinId(candidate.selectedSkin)
    || typeof candidate.followSystem !== 'boolean'
  ) return null;

  return {
    version: THEME_PREFERENCE_VERSION,
    selectedSkin: candidate.selectedSkin,
    followSystem: candidate.followSystem,
  };
}

export function decodeSerializedThemePreference(value: string | null): ThemePreference | null {
  if (value === null) return null;
  try {
    return decodeThemePreference(JSON.parse(value));
  } catch {
    return null;
  }
}

export function decodeThemeSnapshotEnvelope(value: unknown): ThemeSnapshotEnvelope | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const candidate = value as Record<string, unknown>;
  const preference = decodeThemePreference(candidate.preference);
  if (
    candidate.protocolVersion !== THEME_PROTOCOL_VERSION
    || !Number.isSafeInteger(candidate.revision)
    || (candidate.revision as number) < 1
    || !preference
  ) return null;

  return {
    protocolVersion: THEME_PROTOCOL_VERSION,
    revision: candidate.revision as number,
    preference,
  };
}

export function resolveEffectiveTheme(
  preference: ThemePreference,
  systemDark: boolean,
): EffectiveTheme {
  const skin = preference.followSystem && systemDark
    ? 'shanshui-yemo'
    : preference.selectedSkin;
  return {
    skin,
    appearance: skin === 'shanshui-yemo' ? 'dark' : 'light',
  };
}

export function applyEffectiveTheme(root: ThemeRoot, theme: EffectiveTheme): void {
  root.setAttribute('data-skin', theme.skin);
  root.setAttribute('data-appearance', theme.appearance);
}

interface BootstrapThemeOptions {
  readonly root: ThemeRoot;
  readonly storage: ThemeStorage;
  readonly systemDark: boolean;
  readonly repairStorage?: boolean;
  readonly onError?: (error: unknown) => void;
}

export interface ThemeBootstrapResult {
  readonly preference: ThemePreference;
  readonly effectiveTheme: EffectiveTheme;
}

export function bootstrapTheme({
  root,
  storage,
  systemDark,
  repairStorage = true,
  onError = () => undefined,
}: BootstrapThemeOptions): ThemeBootstrapResult {
  let serialized: string | null = null;
  try {
    serialized = storage.getItem(THEME_STORAGE_KEY);
  } catch (error) {
    onError(error);
  }

  const decoded = decodeSerializedThemePreference(serialized);
  const preference = decoded ?? DEFAULT_THEME_PREFERENCE;
  const effectiveTheme = resolveEffectiveTheme(preference, systemDark);
  applyEffectiveTheme(root, effectiveTheme);

  if (!decoded && repairStorage) {
    try {
      storage.setItem(THEME_STORAGE_KEY, JSON.stringify(DEFAULT_THEME_PREFERENCE));
    } catch (error) {
      onError(error);
    }
  }

  return { preference, effectiveTheme };
}
