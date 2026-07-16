import { defineConfig } from 'vite';

const bossServerUrl = process.env.BOSS_SERVER_URL || 'http://127.0.0.1:8787';

const bossServerProxy = {
  '/api/boss': {
    target: bossServerUrl,
    changeOrigin: true
  }
};

export default defineConfig({
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 800
  },
  server: {
    host: '127.0.0.1',
    port: 5181,
    strictPort: true,
    proxy: bossServerProxy
  },
  preview: {
    host: '127.0.0.1',
    port: 4181,
    strictPort: true,
    proxy: bossServerProxy
  }
});
