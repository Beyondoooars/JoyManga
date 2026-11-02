import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import Components from 'unplugin-vue-components/vite'
import { NaiveUiResolver } from 'unplugin-vue-components/resolvers'
import UnoCSS from 'unocss/vite'
import vueJsx from '@vitejs/plugin-vue-jsx'
import vueDevTools from 'vite-plugin-vue-devtools'

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST

// https://vitejs.dev/config/
export default defineConfig(async ({ command }) => ({
  plugins: [
    vue(),
    UnoCSS(),
    vueJsx(),
    ...(command === 'serve' ? [vueDevTools()] : []),
    Components({
      resolvers: [NaiveUiResolver()],
      dts: command === 'serve' ? 'src/components.d.ts' : false,
    }),
  ],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent vite from obscuring rust errors
  clearScreen: false,
  build: {
    // Vite 8 默认目标是 Chrome 111；Android 7 的旧 WebView 可能更低，显式降级以保证可运行。
    target: 'chrome87',
  },
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 5005,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell vite to ignore watching `src-tauri`
      ignored: ['**/src-tauri/**'],
    },
  },
}))
