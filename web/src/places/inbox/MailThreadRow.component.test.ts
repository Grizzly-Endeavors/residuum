import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi, type Mock } from "vitest";
import type { MailThreadSummary } from "../../lib/hub-types";
import { render, screen } from "../../test/component";
import MailThreadRow from "./MailThreadRow.svelte";

const NOW = Date.parse("2026-03-14T12:00:00Z");

function summary(overrides: Partial<MailThreadSummary> = {}): MailThreadSummary {
  return {
    thread_id: "atlas/agent/20260311_wiki_audit",
    subject: "Wiki audit: stale pages",
    participants: ["atlas", "scout"],
    message_count: 4,
    latest_at: "2026-03-14T09:00:00Z",
    snippet: "Thanks both, I'll merge the list.",
    unread: false,
    has_attachments: false,
    in_inbox: true,
    in_sent: true,
    archived: false,
    ...overrides,
  };
}

function row(
  overrides: Partial<MailThreadSummary> = {},
  selected = false,
): { link: HTMLElement; onopen: Mock<(event: MouseEvent) => void> } {
  const onopen = vi.fn((event: MouseEvent) => {
    event.preventDefault();
  });
  render(MailThreadRow, {
    summary: summary(overrides),
    href: "/inbox?thread=x",
    selected,
    now: NOW,
    onopen,
  });
  return { link: screen.getByRole("link"), onopen };
}

describe("MailThreadRow", () => {
  it("shows who, how many messages, when, the subject and the latest line", () => {
    const { link } = row();
    expect(link).toHaveAttribute("href", "/inbox?thread=x");
    expect(link).toHaveTextContent("atlas, scout");
    expect(link).toHaveTextContent("4 messages");
    expect(link).toHaveTextContent("3h ago");
    expect(link).toHaveTextContent("Wiki audit: stale pages");
    expect(link).toHaveTextContent("Thanks both, I'll merge the list.");
    expect(link.querySelector("time")).toHaveAttribute("datetime", "2026-03-14T09:00:00Z");
    expect(link).not.toHaveAttribute("aria-current");
    expect(link).not.toHaveAttribute("data-unread");
  });

  it("marks an unread thread for sight and for screen readers", () => {
    const { link } = row({ unread: true });
    expect(link).toHaveAttribute("data-unread");
    expect(link).toHaveTextContent("Wiki audit: stale pages, unread");
  });

  it("says a thread carries files", () => {
    expect(row({ has_attachments: true }).link).toHaveTextContent("has attachments");
  });

  it("leaves out the count of a single message and an empty snippet", () => {
    const { link } = row({ message_count: 1, snippet: "" });
    expect(link).not.toHaveTextContent("messages");
    expect(link.querySelector(".mail-row-snippet")).toBeNull();
  });

  it("reads (no subject) for a blank subject", () => {
    expect(row({ subject: "  " }).link).toHaveTextContent("(no subject)");
  });

  it("is the current link while its thread is open", () => {
    expect(row({}, true).link).toHaveAttribute("aria-current", "true");
  });

  it("hands a click to the place", async () => {
    const { link, onopen } = row();
    await userEvent.click(link);
    expect(onopen).toHaveBeenCalledTimes(1);
  });
});
