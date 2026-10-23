import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "../../test/component";
import AttachmentChip from "./AttachmentChip.svelte";

describe("AttachmentChip, ready", () => {
  it("is a download link that says what it does and which file, described by the size", () => {
    render(AttachmentChip, { filename: "report.pdf", size: 1_258_291, href: "/files/report.pdf" });
    const link = screen.getByRole("link", { name: "Download report.pdf" });
    expect(link).toHaveAttribute("href", "/files/report.pdf");
    expect(link).toHaveAttribute("download", "report.pdf");
    expect(link).toHaveAccessibleDescription("1.2 MB");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("saves the download under the name it is given", () => {
    render(AttachmentChip, {
      filename: "report.pdf",
      href: "/files/3f9a",
      download: "q3-report.pdf",
    });
    expect(screen.getByRole("link", { name: "Download report.pdf" })).toHaveAttribute(
      "download",
      "q3-report.pdf",
    );
  });

  it("is an Open button that runs its action", async () => {
    const user = userEvent.setup();
    const onopen = vi.fn();
    render(AttachmentChip, { filename: "notes.md", size: 2048, onopen });
    const button = screen.getByRole("button", { name: "Open notes.md" });
    expect(button).toHaveAttribute("type", "button");
    expect(button).toHaveAccessibleDescription("2.0 KB");
    expect(screen.queryByRole("link")).toBeNull();

    await user.click(button);
    button.focus();
    await user.keyboard("{Enter}");
    expect(onopen).toHaveBeenCalledTimes(2);
  });

  it("keeps both when it can open and download: Open is the chip, the download its own link", () => {
    render(AttachmentChip, {
      filename: "restore-log.txt",
      href: "/files/restore-log.txt",
      onopen: () => {},
    });
    expect(screen.getByRole("button", { name: "Open restore-log.txt" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Download restore-log.txt" })).toHaveAttribute(
      "href",
      "/files/restore-log.txt",
    );
  });

  it("is plain text with no control when it has nothing to do", () => {
    render(AttachmentChip, { filename: "receipts.zip", size: 812 });
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByText("812 B")).toBeInTheDocument();
  });

  it("is removed from its own button, without opening it", async () => {
    const user = userEvent.setup();
    const onremove = vi.fn();
    const onopen = vi.fn();
    render(AttachmentChip, { filename: "receipts.zip", onremove, onopen });
    await user.click(screen.getByRole("button", { name: "Remove receipts.zip" }));
    expect(onremove).toHaveBeenCalledTimes(1);
    expect(onopen).not.toHaveBeenCalled();
  });

  it("offers Remove beside a download link", () => {
    render(AttachmentChip, {
      filename: "receipts.zip",
      href: "/files/receipts.zip",
      onremove: () => {},
    });
    expect(screen.getByRole("link", { name: "Download receipts.zip" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove receipts.zip" })).toBeInTheDocument();
  });

  it("shows the size only when it is known", () => {
    const { container } = render(AttachmentChip, { filename: "mystery.bin" });
    expect(container.querySelector(".ui-attachment-size")).toBeNull();
  });

  it.each([
    ["pharmacy-receipt.jpg", undefined, "image"],
    ["scan", "image/png", "image"],
    ["grant-outline-v1.md", undefined, "document"],
    ["budget-2027.xlsx", undefined, "file"],
  ] as const)("draws %s (%s) as a %s", (filename, contentType, kind) => {
    const { container } = render(AttachmentChip, { filename, contentType });
    expect(container.querySelector(".ui-attachment")).toHaveAttribute("data-kind", kind);
  });

  it("reads its name once, and keeps the extension apart so it survives truncation", () => {
    const { container } = render(AttachmentChip, { filename: "lease-renewal-2027-signed.pdf" });
    expect(screen.getByText("lease-renewal-2027-signed.pdf")).toBeInTheDocument();
    const pieces = container.querySelector(".ui-attachment-pieces");
    expect(pieces).toHaveAttribute("aria-hidden", "true");
    expect(pieces?.querySelector(".ui-attachment-stem")).toHaveTextContent(
      "lease-renewal-2027-signed",
    );
    expect(pieces?.querySelector(".ui-attachment-extension")).toHaveTextContent(".pdf");
  });
});

describe("AttachmentChip, pending", () => {
  it("shows a busy indicator named for what is happening to which file", () => {
    render(AttachmentChip, { filename: "report.pdf", state: "pending" });
    const busy = screen.getByRole("progressbar", { name: "Uploading report.pdf" });
    expect(busy).not.toHaveAttribute("aria-valuenow");
  });

  it("names what is happening in the caller's words", () => {
    render(AttachmentChip, { filename: "report.pdf", state: "pending", pendingLabel: "Fetching" });
    expect(screen.getByRole("progressbar", { name: "Fetching report.pdf" })).toBeInTheDocument();
  });

  it("reports progress as a percentage, held between none and all", async () => {
    const { container, rerender } = render(AttachmentChip, {
      filename: "report.pdf",
      state: "pending",
      progress: 0.42,
    });
    const bar = screen.getByRole("progressbar", { name: "Uploading report.pdf" });
    expect(bar).toHaveAttribute("aria-valuemin", "0");
    expect(bar).toHaveAttribute("aria-valuemax", "100");
    expect(bar).toHaveAttribute("aria-valuenow", "42");
    expect(container.querySelector<HTMLElement>(".ui-attachment-progress-fill")).toHaveStyle({
      width: "42%",
    });

    await rerender({ progress: 7 });
    expect(bar).toHaveAttribute("aria-valuenow", "100");
    await rerender({ progress: -1 });
    expect(bar).toHaveAttribute("aria-valuenow", "0");
  });

  it("can't be opened or downloaded yet, but can be removed", async () => {
    const user = userEvent.setup();
    const onremove = vi.fn();
    render(AttachmentChip, {
      filename: "report.pdf",
      state: "pending",
      href: "/files/report.pdf",
      onopen: () => {},
      onremove,
    });
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.queryByRole("button", { name: "Open report.pdf" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Remove report.pdf" }));
    expect(onremove).toHaveBeenCalledTimes(1);
  });

  it("becomes a download link once the file is ready", async () => {
    const { rerender } = render(AttachmentChip, {
      filename: "report.pdf",
      state: "pending",
      href: "/files/report.pdf",
    });
    expect(screen.queryByRole("link")).toBeNull();
    await rerender({ state: "ready" });
    expect(screen.getByRole("link", { name: "Download report.pdf" })).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });
});

describe("AttachmentChip, failed", () => {
  it("announces the failure with the file it belongs to", () => {
    render(AttachmentChip, {
      filename: "report.pdf",
      state: "failed",
      failure: "Couldn't upload",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("report.pdf: Couldn't upload");
  });

  it("says it failed unless told more", () => {
    render(AttachmentChip, { filename: "report.pdf", state: "failed" });
    expect(screen.getByRole("alert")).toHaveTextContent("report.pdf: Failed");
  });

  it("offers Retry for that file, and runs it", async () => {
    const user = userEvent.setup();
    const onretry = vi.fn();
    render(AttachmentChip, { filename: "report.pdf", state: "failed", onretry });
    const retry = screen.getByRole("button", { name: "Retry report.pdf" });
    expect(retry).toHaveTextContent("Retry");
    await user.click(retry);
    expect(onretry).toHaveBeenCalledTimes(1);
  });

  it("offers no Retry without an action to run, and none unless it failed", () => {
    const failed = render(AttachmentChip, { filename: "report.pdf", state: "failed" });
    expect(screen.queryByRole("button", { name: /Retry/ })).toBeNull();
    failed.unmount();

    render(AttachmentChip, { filename: "report.pdf", onretry: () => {} });
    expect(screen.queryByRole("button", { name: /Retry/ })).toBeNull();
  });

  it("can't be opened or downloaded, but can be removed", async () => {
    const user = userEvent.setup();
    const onremove = vi.fn();
    render(AttachmentChip, {
      filename: "report.pdf",
      state: "failed",
      href: "/files/report.pdf",
      onopen: () => {},
      onremove,
    });
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.queryByRole("button", { name: "Open report.pdf" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Remove report.pdf" }));
    expect(onremove).toHaveBeenCalledTimes(1);
  });

  it("shows the failure in place of the size", () => {
    const { container } = render(AttachmentChip, {
      filename: "report.pdf",
      size: 1024,
      state: "failed",
    });
    expect(container.querySelector(".ui-attachment-size")).toBeNull();
    expect(container.querySelector(".ui-attachment")).toHaveAttribute("data-state", "failed");
  });
});
