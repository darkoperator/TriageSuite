import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// The Tauri dev server URL (tauri.conf.json `build.devUrl`) expects port 5173.
// `__OXL_MOCK__` is a compile-time constant: true only for the dev server started in `mock` mode (`npm run
// dev:mock`). Every other command and mode (`vite build` in any env, `vite` with the default mode, Vitest) compiles it
// to `false`, so the mock backend can neither run nor be bundled outside that one dev server. No environment variable
// is read.
export default defineConfig(({ command, mode }) => ({
  plugins: [react()],
  clearScreen: false,
  define: { __OXL_MOCK__: JSON.stringify(command === "serve" && mode === "mock") },
  server: { port: 5173, strictPort: true },
  build: { outDir: "dist", emptyOutDir: true },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    // The slowest tests (ESLint's Node API, the 5,000-row mock) take a few seconds: Vitest's default 5 s fails them
    // when a build is using the CPU.
    testTimeout: 20_000,
  },
}));
