import type { MermaidConfig } from 'mermaid';
import type { SkinId, ThemeAppearance } from './theme';

interface MermaidPalette {
  readonly background: string;
  readonly border: string;
  readonly line: string;
  readonly primary: string;
  readonly primaryText: string;
  readonly secondary: string;
  readonly secondaryText: string;
  readonly surface: string;
  readonly text: string;
}

const MERMAID_PALETTES: Readonly<Record<SkinId, MermaidPalette>> = Object.freeze({
  'jinxiu-zhusha': { background: '#FFFFFF', border: '#C9CCC7', line: '#302D2A', primary: '#A32638', primaryText: '#FFF9F3', secondary: '#2F6F78', secondaryText: '#FFF9F3', surface: '#F7F6F3', text: '#202421' },
  'ruyao-tianqing': { background: '#FCFDFC', border: '#C0CBC4', line: '#303733', primary: '#2F6F62', primaryText: '#FFF8F5', secondary: '#6D5B78', secondaryText: '#FFF8F5', surface: '#F1F5F2', text: '#1F2A24' },
  'qinghua-jilan': { background: '#FFFFFF', border: '#C6CCD5', line: '#2A313A', primary: '#235BA8', primaryText: '#FFF9F4', secondary: '#3F705B', secondaryText: '#FFF9F4', surface: '#F4F6F9', text: '#1D2530' },
  'songke-zhuying': { background: '#FEFEFC', border: '#C5CBBF', line: '#20231F', primary: '#3E6B4F', primaryText: '#FFF8F2', secondary: '#8B6B34', secondaryText: '#FFF8F2', surface: '#F3F5F0', text: '#1F2721' },
  'shanshui-yemo': { background: '#202421', border: '#566159', line: '#A7B0A9', primary: '#477968', primaryText: '#FFF8F2', secondary: '#C6A66A', secondaryText: '#1E1A12', surface: '#262B28', text: '#F0F2EE' },
});

export interface MermaidThemeConfig {
  readonly darkMode: boolean;
  readonly theme: 'base';
  readonly themeVariables: NonNullable<MermaidConfig['themeVariables']>;
}

export function getMermaidThemeConfig(
  skin: SkinId,
  appearance: ThemeAppearance,
): MermaidThemeConfig {
  const palette = MERMAID_PALETTES[skin];
  return {
    darkMode: appearance === 'dark',
    theme: 'base',
    themeVariables: {
      background: palette.background,
      clusterBkg: palette.surface,
      clusterBorder: palette.border,
      edgeLabelBackground: palette.background,
      lineColor: palette.line,
      mainBkg: palette.primary,
      nodeBorder: palette.border,
      primaryBorderColor: palette.border,
      primaryColor: palette.primary,
      primaryTextColor: palette.primaryText,
      secondaryBorderColor: palette.border,
      secondaryColor: palette.secondary,
      secondaryTextColor: palette.secondaryText,
      tertiaryBorderColor: palette.border,
      tertiaryColor: palette.surface,
      tertiaryTextColor: palette.text,
      textColor: palette.text,
    },
  };
}
