import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

const rootDir = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json-summary', 'html'],
      reportsDirectory: './coverage',
      thresholds: {
        lines: 15,
        statements: 15,
        functions: 50,
        branches: 70,
      },
      exclude: [
        '**/*.test.{ts,tsx}',
        '**/*.d.ts',
        'src/test/**',
      ],
    },
  },
  optimizeDeps: {
    entries: ['index.html'],
  },
  build: {
    outDir: fileURLToPath(new URL('../desktop-dist', import.meta.url)),
    emptyOutDir: true,
    chunkSizeWarningLimit: 1200,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes('node_modules')) return undefined
          if (id.includes('/react/') || id.includes('/react-dom/') || id.includes('/scheduler/')) {
            return 'vendor-react'
          }
          if (id.includes('/@codemirror/') || id.includes('/@uiw/') || id.includes('/@lezer/')) {
            return 'vendor-editor'
          }
          if (id.includes('/react-markdown/') || id.includes('/remark-gfm/') || id.includes('/tippy.js/')) {
            return 'vendor-content'
          }
          if (id.includes('/@xyflow/')) return 'vendor-flow'
          if (id.includes('/lucide-react/') || id.includes('/clsx/') || id.includes('/tailwind-merge/')) {
            return 'vendor-ui'
          }
          return undefined
        },
      },
    },
  },
  resolve: {
    alias: {
      '@': `${rootDir}src/vendor/freecut`,
      '@redbox': `${rootDir}src`,
      '@tauri-apps/api/core': `${rootDir}src/compat/tauri-core.ts`,
      '@tauri-apps/api/event': `${rootDir}src/compat/tauri-event.ts`,
    },
  },
})
