import react from '@vitejs/plugin-react';
import { configDefaults, defineConfig } from 'vitest/config';
import { createExcalidrawFontTransformPlugin } from './scripts/excalidraw-assets.mjs';
import { createPdfAssetManifestPlugin } from './scripts/pdf-assets.mjs';

export default defineConfig({
  plugins: [createPdfAssetManifestPlugin(), createExcalidrawFontTransformPlugin(), react()],
  optimizeDeps: {
    exclude: ['@excalidraw/excalidraw'],
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  test: {
    exclude: [...configDefaults.exclude, '.omx/**'],
    server: {
      deps: {
        inline: ['@excalidraw/excalidraw', 'open-color'],
      },
    },
  },
});
