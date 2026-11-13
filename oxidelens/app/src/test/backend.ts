// How tests stand in for the desktop app. `setup.ts` mocks `@tauri-apps/api/core` and `/event` for every test file:
//
// - `invoke` records the call in `callLog`, then answers from `overrideCommand(cmd, fn)` when a test set one, else
//   from the mock backend (the same one `npm run dev:mock` serves), so a screen test sees realistic answers;
// - `listen` hooks the mock backend's event bus, and `emitEvent` sends an event down it.
//
// Everything is reset before each test (`resetBackend`): the log, the overrides, the mock's case and its listeners.
// Tests never mock `api.ts` itself: the screens under test call the real wrappers.
import type { AppEvent } from "../contract/generated/commands";
import { mockCall, mockEmit, mockOn, resetMock, setMockLatency } from "../mock/backend";

type Args = Record<string, unknown> | undefined;

/** Every `invoke` of the current test, in order. */
export const callLog: { cmd: string; args: Args }[] = [];

const overrides = new Map<string, (args: Args) => unknown>();

/** Answers `cmd` with `fn`'s result (a rejected promise or a throw rejects the call) for the rest of the test. */
export function overrideCommand(cmd: string, fn: (args: Args) => unknown): void {
  overrides.set(cmd, fn);
}

/** The calls of one command, for `expect(callsOf("grid_page")).toHaveLength(1)`. */
export const callsOf = (cmd: string) => callLog.filter((c) => c.cmd === cmd);

export async function invoke(cmd: string, args?: Args): Promise<unknown> {
  callLog.push({ cmd, args });
  const override = overrides.get(cmd);
  if (override) return override(args);
  return mockCall(cmd, args);
}

/** What `listen` of `@tauri-apps/api/event` does in tests: the handler gets Tauri's `{ event, id, payload }`. */
export async function listen(event: string, handler: (e: { event: string; id: number; payload: unknown }) => void) {
  return mockOn(event as AppEvent, (payload) => handler({ event, id: 0, payload }));
}

/** Sends an event to every listener, as the app does. */
export const emitEvent = mockEmit;

export function resetBackend(): void {
  callLog.length = 0;
  overrides.clear();
  resetMock();
  setMockLatency(0);
}
