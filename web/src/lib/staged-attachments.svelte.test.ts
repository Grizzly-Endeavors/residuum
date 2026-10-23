import { afterEach, describe, expect, it, vi } from "vitest";
import { StagedAttachment } from "./staged-attachments.svelte";

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** Each upload waits until the test answers it, in the order they were asked. */
function heldUploads(): {
  answer: (index: number, response: Response) => void;
  count: () => number;
} {
  const pending: ((response: Response) => void)[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(
      () =>
        new Promise<Response>((resolve) => {
          pending.push(resolve);
        }),
    ),
  );
  return {
    answer: (index, response) => {
      pending[index]?.(response);
    },
    count: () => pending.length,
  };
}

function report(): StagedAttachment {
  return new StagedAttachment(new File(["%PDF"], "report.pdf", { type: "application/pdf" }));
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("uploading a staged file again", () => {
  it("uploads an uploaded file again, and only the new id counts", async () => {
    const uploads = heldUploads();
    const attachment = report();
    const first = attachment.upload();
    uploads.answer(0, json({ id: "upload_1", filename: "report.pdf", size: 4 }));
    await first;
    expect(attachment.uploadId).toBe("upload_1");

    const again = attachment.uploadAgain();
    expect(attachment.status).toBe("uploading");
    expect(attachment.uploadId).toBeNull();
    uploads.answer(1, json({ id: "upload_2", filename: "report.pdf", size: 4 }));
    await again;
    expect(attachment.status).toBe("uploaded");
    expect(attachment.uploadId).toBe("upload_2");
  });

  it("lets nothing the attempt it replaced answers later count", async () => {
    const uploads = heldUploads();
    const attachment = report();
    const first = attachment.upload();
    const again = attachment.uploadAgain();
    expect(uploads.count()).toBe(2);

    uploads.answer(1, json({ id: "upload_2", filename: "report.pdf", size: 4 }));
    await again;
    uploads.answer(0, json({ error: "disk full" }, 500));
    await first;
    expect(attachment.status).toBe("uploaded");
    expect(attachment.uploadId).toBe("upload_2");
  });

  it("does nothing for a file taken off the message", () => {
    const uploads = heldUploads();
    const attachment = report();
    void attachment.upload();
    attachment.remove();
    void attachment.uploadAgain();
    expect(uploads.count()).toBe(1);
  });
});
