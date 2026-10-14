import { describe, expect, it, vi } from "vitest";
import { carriesFiles, refuseStrayFileDrops } from "./file-drop";

function dragEvent(
  type: string,
  types: string[],
  defaultPrevented = false,
): { event: DragEvent; preventDefault: ReturnType<typeof vi.fn> } {
  const preventDefault = vi.fn();
  const event = {
    type,
    defaultPrevented,
    dataTransfer: { types, dropEffect: "copy" },
    preventDefault,
  } as unknown as DragEvent;
  return { event, preventDefault };
}

describe("carriesFiles", () => {
  it("is true for files from outside the page, and false for text or nothing", () => {
    expect(carriesFiles(dragEvent("dragover", ["Files"]).event)).toBe(true);
    expect(carriesFiles(dragEvent("dragover", ["text/plain", "Files"]).event)).toBe(true);
    expect(carriesFiles(dragEvent("dragover", ["text/plain"]).event)).toBe(false);
    expect(carriesFiles({ dataTransfer: null } as unknown as DragEvent)).toBe(false);
  });
});

describe("refuseStrayFileDrops", () => {
  it("stops the browser opening a file dropped where nothing takes it", () => {
    const { event, preventDefault } = dragEvent("drop", ["Files"]);
    refuseStrayFileDrops(event);
    expect(preventDefault).toHaveBeenCalledOnce();
  });

  it("shows a file over a place that doesn't take it as undroppable", () => {
    const { event, preventDefault } = dragEvent("dragover", ["Files"]);
    refuseStrayFileDrops(event);
    expect(preventDefault).toHaveBeenCalledOnce();
    expect(event.dataTransfer?.dropEffect).toBe("none");
  });

  it("leaves a drop that a place took, and drags that aren't files, alone", () => {
    const taken = dragEvent("dragover", ["Files"], true);
    refuseStrayFileDrops(taken.event);
    expect(taken.preventDefault).not.toHaveBeenCalled();
    expect(taken.event.dataTransfer?.dropEffect).toBe("copy");

    const text = dragEvent("drop", ["text/plain"]);
    refuseStrayFileDrops(text.event);
    expect(text.preventDefault).not.toHaveBeenCalled();
  });
});
