import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react()],
  server: {
    port: 3400,
    proxy: {
      '/api': {
        target: 'http://172.16.208.113:3300',
        changeOrigin: true,
      },
    },
  },
})
