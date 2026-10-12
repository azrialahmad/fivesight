import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { resolve } from 'node:path';

export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: 'chrome120',
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        live: resolve(__dirname, 'live.html')
      }
    }
  }
});
