import { beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "./toast.svelte";
import { notifyWithUndo } from "./undo";

/**
 * The checkpoint id the delete/revoke response handed back for the action.
 * A later checkpoint (`newer-cp`) lands before Undo is clicked; listing the
 * repo would now return that newer id first.
 */
const ACTION_CHECKPOINT = "action-cp";
const NEWER_CHECKPOINT = "newer-cp";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.href;
  return input.url;
}

describe("toast undo targets the action's own checkpoint", () => {
  const restored: string[] = [];

  beforeEach(() => {
    restored.length = 0;
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = requestUrl(input);
        const method = init?.method ?? "GET";
        // What `undoLastAction` used to do: ask for the newest checkpoint
        // at click time. A checkpoint taken after the action is now first.
        if (method === "GET" && url.startsWith("/api/checkpoints?")) {
          return jsonResponse({
            items: [
              {
                id: NEWER_CHECKPOINT,
                timestamp: "2026-09-26T03:00:00Z",
                address: "system",
                run_id: null,
                turn_id: null,
                trigger: "turn_end",
                summary: "later turn",
                changed_path_count: 1,
              },
            ],
            next_cursor: null,
          });
        }
        if (method === "POST" && url.includes("/restore")) {
          const id = decodeURIComponent(url.split("/api/checkpoints/")[1]?.split("/")[0] ?? "");
          restored.push(id);
          return jsonResponse({
            checkpoint_id: "after-restore",
            restored_paths: ["agent-keys.toml.enc"],
          });
        }
        return new Response(`unexpected ${method} ${url}`, { status: 500 });
      }),
    );
  });

  it("restores the action's checkpoint when a newer one lands before Undo is clicked", async () => {
    notifyWithUndo("Removed github_token.", "config", "agent-keys.toml.enc", ACTION_CHECKPOINT);
    const shown = [...toast.toasts.values()].at(-1);
    expect(shown?.action?.label).toBe("Undo");

    shown?.action?.onClick();
    await vi.waitFor(() => {
      expect(restored.length).toBeGreaterThan(0);
    });

    expect(restored).toEqual([ACTION_CHECKPOINT]);
  });

  it("does not offer Undo when no checkpoint id came back", () => {
    notifyWithUndo("Removed github_token.", "config", "agent-keys.toml.enc", null);
    const shown = [...toast.toasts.values()].at(-1);
    expect(shown).toMatchObject({ kind: "success", message: "Removed github_token." });
    expect(shown?.action).toBeUndefined();
  });
});
