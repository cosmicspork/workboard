import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  // Relative URLs, so the build works at / standalone and under Tracon's /t/<token>/ prefix.
  base: './',
  plugins: [svelte()],
  build: { outDir: 'dist', emptyOutDir: true }
});
