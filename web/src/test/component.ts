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

/** Flush the fetch microtasks a component started, and any 0ms timers. */
export async function settle(): Promise<void> {
  await act(async () => {
    if (vi.isFakeTimers()) {
      await vi.advanceTimersByTimeAsync(0);
    }
  });
}

/** Move the faked clock forward and let Svelte paint what the poll found. */
export async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
