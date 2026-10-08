import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// The compiled engine (src/wasm/*, see scripts/build-wasm.sh) is imported
// with an explicit `?url` asset reference — no WASM bundler plugins needed.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    host: '0.0.0.0',
    port: 5173,
    // Dev-only: accept tunneled preview hosts (sandbox proxies, tunnels).
    allowedHosts: true,
  },
});
