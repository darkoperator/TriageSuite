import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach, vi } from "vitest";

// The app's one door to the backend (`api.ts`) goes to the mock backend in every test, with a call log and per-test
// overrides: see `./backend.ts`. The factories import lazily so the mock is loaded once per test file.
vi.mock("@tauri-apps/api/core", async () => ({ invoke: (await import("./backend")).invoke }));
vi.mock("@tauri-apps/api/event", async () => ({ listen: (await import("./backend")).listen }));

beforeEach(async () => {
  (await import("./backend")).resetBackend();
});

// Vitest globals are off, so Testing Library cannot register its own cleanup.
afterEach(() => {
  cleanup();
});
