/**
 * Helpers for Svelte component tests.
 *
 * Put a test next to the component under `src/components/`, or name it
 * `*.component.test.ts`. Those files run in jsdom. `npm test` also runs the
 * Node unit tests under `src/lib/`.
 */
import { act } from "@testing-library/svelte";
import { vi } from "vitest";

export { fireEvent, render, screen } from "@testing-library/svelte";

/** JSON body the real `apiFetch` helper can parse. */
export function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

export type FetchHandler = (
  url: string,
  init: RequestInit | undefined,
) => Response | Promise<Response>;

/**
 * Replace `fetch` for one test. The handler sees the request URL the component
 * actually called, so a test can answer one endpoint and reject the rest.
 */
export function mockFetch(handler: FetchHandler): void {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      return Promise.resolve(handler(requestUrl(input), init));
    }),
  );
}

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.href;
  return input.url;
}

/**
 * Flush the fetch microtasks a component started, and any 0ms timers.
 *
 * A mocked `fetch` answers at once, but reading the `Response` body takes a
 * number of microtask turns that depends on the undici bundled with Node: a
 * fixed count of turns that covers one Node misses on another. A macrotask
 * runs only once the microtask queue is empty, so waiting for one lets every
 * chain the component started finish, however deep. Under fake timers,
 * `advanceTimersByTimeAsync` yields to the real event loop the same way.
 */
export async function settle(): Promise<void> {
  await act(async () => {
    if (vi.isFakeTimers()) {
      await vi.advanceTimersByTimeAsync(0);
    } else {
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
  });
}

/** Move the faked clock forward and let Svelte paint what the poll found. */
export async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

/**
 * Replace `WebSocket` with one that never connects. Choosing an agent
 * (`setViewedAgent`) opens the agent connection, and a component test has
 * no server to answer it.
 */
export function stubWebSocket(): void {
  class SilentWebSocket {
    static readonly OPEN = 1;
    readonly readyState = 0;
    onopen: (() => void) | null = null;
    onmessage: (() => void) | null = null;
    onclose: (() => void) | null = null;
    send(): void {}
    close(): void {}
  }
  vi.stubGlobal("WebSocket", SilentWebSocket);
}

/**
 * Give jsdom, which has no `matchMedia`, a pointer: a mouse, or a touch
 * screen (`(pointer: coarse)`) while `touch` says so. Every other media
 * query doesn't match.
 */
export function stubPointer(touch: () => boolean = () => false): void {
  vi.stubGlobal("matchMedia", (media: string) => ({
    matches: touch() && media === "(pointer: coarse)",
    media,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
}
