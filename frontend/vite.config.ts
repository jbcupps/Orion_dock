import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 3000,
    proxy: {
      '/api': {
        target: 'http://localhost:8080',
        changeOrigin: true,
        // Pro council can run multiple LLM calls; 3 min timeout prevents 502.
        timeout: 180000,
      },
      '/health': { target: 'http://localhost:8080', changeOrigin: true },
      '/ready': { target: 'http://localhost:8080', changeOrigin: true },
    },
  },
});
