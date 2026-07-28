import {
  FONT_FAMILY,
  exportToSvg,
  getNonDeletedElements,
  restore,
} from '@excalidraw/excalidraw';
import { parseExcalidrawScene } from './excalidrawScene';

export function restoreExcalidrawScene(content: string): ReturnType<typeof restore> {
  const scene = parseExcalidrawScene(content);
  return restore({
    ...scene,
    appState: {
      ...scene.appState,
      currentItemFontFamily: typeof scene.appState.currentItemFontFamily === 'number'
        ? scene.appState.currentItemFontFamily
        : FONT_FAMILY.Excalifont,
      viewBackgroundColor: typeof scene.appState.viewBackgroundColor === 'string'
        ? scene.appState.viewBackgroundColor
        : 'transparent',
    },
  } as unknown as Parameters<typeof restore>[0], null, null, { repairBindings: true });
}

export async function exportExcalidrawSceneSvg(
  content: string,
  appearance: 'light' | 'dark',
): Promise<SVGSVGElement> {
  const restored = restoreExcalidrawScene(content);
  return exportToSvg({
    elements: getNonDeletedElements(restored.elements),
    appState: {
      ...restored.appState,
      exportBackground: false,
      exportEmbedScene: false,
      exportWithDarkMode: appearance === 'dark',
      viewBackgroundColor: 'transparent',
    },
    files: restored.files,
    renderEmbeddables: false,
    reuseImages: true,
  });
}
