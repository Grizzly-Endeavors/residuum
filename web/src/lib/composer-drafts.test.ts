import { afterEach, describe, expect, it, vi } from "vitest";
import { readDraft, readDraftImages, saveDraft, saveDraftImages } from "./composer-drafts";

/** A `localStorage` that works, which the Node test environment lacks. */
function stubStorage(): Map<string, string> {
  const items = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => {
      items.set(key, value);
    },
    removeItem: (key: string) => {
      items.delete(key);
    },
  });
  return items;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("composer drafts", () => {
  it("keeps each agent's text apart, and forgets it once it is empty", () => {
    const items = stubStorage();
    saveDraft("atlas", "Check the wiki");
    saveDraft("scout", "Any news?");
    expect(readDraft("atlas")).toBe("Check the wiki");
    expect(readDraft("scout")).toBe("Any news?");

    saveDraft("atlas", "");
    expect(readDraft("atlas")).toBe("");
    expect([...items.keys()]).toEqual(["residuum-draft:scout"]);
  });

  it("starts empty, and doesn't throw, when storage is blocked", () => {
    const refuse = (): never => {
      throw new DOMException("The operation is insecure.", "SecurityError");
    };
    vi.stubGlobal("localStorage", { getItem: refuse, setItem: refuse, removeItem: refuse });
    expect(() => {
      saveDraft("atlas", "lost");
    }).not.toThrow();
    expect(readDraft("atlas")).toBe("");
  });

  it("keeps attached images per agent for the page's life", () => {
    const image = { media_type: "image/png", data: "AAAA" };
    saveDraftImages("atlas", [image]);
    expect(readDraftImages("atlas")).toEqual([image]);
    expect(readDraftImages("scout")).toEqual([]);
    saveDraftImages("atlas", []);
    expect(readDraftImages("atlas")).toEqual([]);
  });
});
