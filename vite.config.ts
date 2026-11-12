import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: 'localhost',
    port: 1420,
    strictPort: true,
    // Cargo writes generated HTML below target; only Tauri should watch Rust builds.
    watch: { ignored: ['**/target/**', '**/src-tauri/**', '**/crates/**'] },
  },
  envPrefix: 'VITE_',
  build: { target: ['es2021', 'chrome105', 'safari15'] },
});
