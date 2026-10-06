import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';

/**
 * The desktop build serves the *same* React tree as the web build.
 *
 * `../vectra-web/src` is aliased rather than copied: a second copy of the UI
 * would be a second place for the engine boundary to drift, and Task 10.0's
 * first rule is that nothing about the engine changes for the desktop — so the
 * components must not fork either. The dev server runs on **5174** (the web
 * app keeps 5173) and `strictPort` keeps Tauri's `devUrl` honest: if the port
 * is taken, the build fails loudly instead of pointing the window at the wrong
 * server.
 */
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@vectra/web': resolve(__dirname, '../vectra-web/src'),
    },
  },
  server: {
    host: '0.0.0.0',
    port: 5174,
    strictPort: true,
    // Tauri's window and the hosted preview both reach this server by DNS name,
    // not by `localhost`.
    allowedHosts: true,
    // Vite refuses to serve files outside its root by default; the shared UI
    // lives one directory up.
    fs: { allow: [resolve(__dirname, '..')] },
    watch: {
      // Never watch the Rust build output. `src-tauri/target` holds hundreds of
      // thousands of files while a Tauri build runs, and a watcher that tries to
      // follow them dies with `ENOSPC` on `fs.watch` (inotify watches are a
      // finite resource) — which is exactly what happened the first time the
      // release build ran alongside this server.
      ignored: ['**/src-tauri/**', '**/target/**', '**/dist/**'],
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'esnext',
  },
});
