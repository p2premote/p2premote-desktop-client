import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [vue()],

  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes('node_modules/element-plus/')) return 'element-plus';
          if (id.includes('node_modules/@element-plus/icons-vue/')) return 'element-plus-icons';
          if (id.includes('node_modules/vue/') || id.includes('node_modules/@vue/')) return 'vue';
          if (id.includes('node_modules/vue-i18n/') || id.includes('node_modules/@intlify/')) return 'vue-i18n';
          if (id.includes('node_modules/vue-router/') || id.includes('node_modules/pinia/')) return 'vue-ecosystem';
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
