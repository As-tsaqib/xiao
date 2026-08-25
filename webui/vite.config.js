import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The module must remain installable offline.  React is bundled into the
// KernelSU WebUI asset rather than fetched from a CDN at device runtime.
export default defineConfig({
  plugins: [react()],
  base: './',
  build: {
    outDir: '../module/webroot',
    emptyOutDir: false,
    cssCodeSplit: false,
    sourcemap: false,
    rollupOptions: {
      output: {
        entryFileNames: 'assets/app.js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: asset => asset.name?.endsWith('.css') ? 'assets/app.css' : 'assets/[name][extname]'
      }
    }
  }
});
