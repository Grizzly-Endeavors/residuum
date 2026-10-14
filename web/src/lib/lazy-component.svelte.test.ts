import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LazyComponent } from "./lazy-component.svelte";
import { notifications } from "./notifications.svelte";
import { waitFor } from "../test/wait";

/** A stand-in for a component's compiled module; the test never mounts it. */
const FAKE = (() => undefined) as unknown as Component<object>;

beforeEach(() => {
  notifications.clear();
  // The raw error goes to the console for developers; the test checks what the person sees.
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("LazyComponent", () => {
  it("holds nothing until something asks for it", () => {
    const load = vi.fn(() => Promise.resolve({ default: FAKE }));
    const lazy = new LazyComponent(load, "Settings");
    expect(lazy.component).toBeNull();
    expect(load).not.toHaveBeenCalled();
  });

  it("fetches the code once, however many times it is asked for", async () => {
    const load = vi.fn(() => Promise.resolve({ default: FAKE }));
    const lazy = new LazyComponent(load, "Settings");
    lazy.ensure();
    lazy.ensure();
    await waitFor(() => {
      expect(lazy.component).toBe(FAKE);
    });
    lazy.ensure();
    expect(load).toHaveBeenCalledOnce();
    expect(lazy.failed).toBe(false);
  });

  it("says it is loading while the code is on its way", async () => {
    let arrive: (module: { default: Component<object> }) => void = () => undefined;
    const lazy = new LazyComponent<object>(
      () =>
        new Promise((resolve) => {
          arrive = resolve;
        }),
      "Settings",
    );
    lazy.ensure();
    expect(lazy.loading).toBe(true);
    arrive({ default: FAKE });
    await waitFor(() => {
      expect(lazy.loading).toBe(false);
    });
    expect(lazy.component).toBe(FAKE);
  });

  it("tells the person when the code can't be fetched, and puts away what asked for it", async () => {
    const lazy = new LazyComponent<object>(
      () => Promise.reject(new TypeError("Failed to fetch dynamically imported module")),
      "Settings",
    );
    const onFailure = vi.fn();
    lazy.ensure(onFailure);
    await waitFor(() => {
      expect(onFailure).toHaveBeenCalledOnce();
    });
    expect(lazy.failed).toBe(true);
    expect(lazy.component).toBeNull();
    expect(notifications.history[0]).toMatchObject({
      kind: "error",
      message: expect.stringMatching(/^Couldn't open Settings\..*reload the page\.$/) as string,
    });
  });

  it("tries again the next time it is asked, and the retry can succeed", async () => {
    const load = vi
      .fn<() => Promise<{ default: Component<object> }>>()
      .mockRejectedValueOnce(new TypeError("offline"))
      .mockResolvedValueOnce({ default: FAKE });
    const lazy = new LazyComponent(load, "the file");
    lazy.ensure();
    await waitFor(() => {
      expect(lazy.failed).toBe(true);
    });

    lazy.ensure();
    await waitFor(() => {
      expect(lazy.component).toBe(FAKE);
    });
    expect(lazy.failed).toBe(false);
    expect(load).toHaveBeenCalledTimes(2);
  });
});
