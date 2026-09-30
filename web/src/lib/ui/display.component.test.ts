import userEvent from "@testing-library/user-event";
import type { Snippet } from "svelte";
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "../../test/component";
import { htmlSnippet } from "../../test/snippets";
import Badge from "./Badge.svelte";
import Banner from "./Banner.svelte";
import EmptyState from "./EmptyState.svelte";
import Kbd from "./Kbd.svelte";
import Skeleton from "./Skeleton.svelte";
import StatusDot from "./StatusDot.svelte";

const text = (value: string): Snippet => htmlSnippet(`<span>${value}</span>`);

describe("Badge", () => {
  it("shows a count, capped at 99+", async () => {
    const { container, rerender } = render(Badge, { count: 7 });
    expect(container).toHaveTextContent("7");
    await rerender({ count: 120 });
    expect(container).toHaveTextContent("99+");
  });

  it("caps at its own maximum", () => {
    const { container } = render(Badge, { count: 12, max: 9 });
    expect(container).toHaveTextContent("9+");
  });

  it("draws nothing for zero", () => {
    const { container } = render(Badge, { count: 0 });
    expect(container.querySelector(".ui-badge")).toBeNull();
  });

  it("reads the exact count with its label, hiding the capped figure", () => {
    const { container } = render(Badge, { count: 120, label: "unread", solid: true });
    const badge = container.querySelector(".ui-badge");
    expect(badge).toHaveAttribute("data-solid", "true");
    expect(badge?.querySelector('[aria-hidden="true"]')).toHaveTextContent("99+");
    expect(screen.getByText("120 unread")).toBeInTheDocument();
  });

  it("fills solid only in the accent tone", () => {
    const { container } = render(Badge, { count: 2, tone: "danger", solid: true });
    expect(container.querySelector(".ui-badge")).not.toHaveAttribute("data-solid");
  });

  it("shows a state label with a dot in its tone", () => {
    const { container } = render(Badge, {
      tone: "positive",
      dot: true,
      children: text("Connected"),
    });
    const badge = container.querySelector(".ui-badge");
    expect(badge).toHaveAttribute("data-tone", "positive");
    expect(badge).toHaveTextContent("Connected");
    expect(badge?.querySelector(".ui-badge-dot")).toHaveAttribute("aria-hidden", "true");
  });
});

describe("StatusDot", () => {
  it("is decorative without a label", () => {
    const { container } = render(StatusDot, { state: "stopped" });
    const dot = container.querySelector(".ui-status-dot");
    expect(dot).toHaveAttribute("aria-hidden", "true");
    expect(dot).toHaveAttribute("data-state", "stopped");
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("is an image named by its label", () => {
    render(StatusDot, { state: "failed", label: "brittle, failed" });
    expect(screen.getByRole("img", { name: "brittle, failed" })).toHaveAttribute(
      "data-state",
      "failed",
    );
  });

  it.each(["running", "stopped", "failed", "starting", "stopping"] as const)(
    "draws the %s state",
    (state) => {
      const { container } = render(StatusDot, { state });
      expect(container.querySelector(`[data-state="${state}"]`)).not.toBeNull();
    },
  );

  it("marks working only on a running agent", () => {
    const running = render(StatusDot, { state: "running", working: true });
    expect(running.container.querySelector("[data-working]")).not.toBeNull();
    running.unmount();
    const stopped = render(StatusDot, { state: "stopped", working: true });
    expect(stopped.container.querySelector("[data-working]")).toBeNull();
  });
});

describe("EmptyState", () => {
  it("is one line of text by default, with its action", () => {
    render(EmptyState, {
      children: text("Nothing running."),
      actions: htmlSnippet("<button>Start a session</button>"),
    });
    expect(screen.getByText("Nothing running.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start a session" })).toBeInTheDocument();
    expect(screen.queryByRole("heading")).toBeNull();
  });

  it("stands in for a place with a title at the given level", () => {
    render(EmptyState, {
      variant: "block",
      icon: "inbox",
      title: "You're all caught up",
      headingLevel: 2,
      children: text("Agents put things here that need you."),
    });
    expect(screen.getByRole("heading", { level: 2, name: "You're all caught up" })).toBeVisible();
    expect(screen.getByText("Agents put things here that need you.")).toBeInTheDocument();
  });
});

describe("Skeleton", () => {
  it("is silent unless labelled", () => {
    const { container } = render(Skeleton, { lines: 3 });
    expect(container.querySelectorAll(".ui-skeleton-bone")).toHaveLength(3);
    expect(screen.queryByRole("status")).toBeNull();
    for (const bone of container.querySelectorAll(".ui-skeleton-bone")) {
      expect(bone).toHaveAttribute("aria-hidden", "true");
    }
  });

  it("announces what is loading once labelled", () => {
    render(Skeleton, { shape: "block", label: "Loading sessions" });
    expect(screen.getByRole("status")).toHaveTextContent("Loading sessions");
  });
});

describe("Banner", () => {
  it("announces errors as alerts and other tones politely", () => {
    const { unmount } = render(Banner, {
      tone: "error",
      title: "Couldn't load sessions.",
      children: text("Try again."),
    });
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn't load sessions. Try again.");
    unmount();

    for (const tone of ["info", "warn"] as const) {
      const banner = render(Banner, { tone, children: text("Reconnecting.") });
      expect(screen.getByRole("status")).toHaveAttribute("data-tone", tone);
      banner.unmount();
    }
  });

  it("offers a dismiss button when it can be dismissed", async () => {
    const user = userEvent.setup();
    const ondismiss = vi.fn();
    render(Banner, { ondismiss, children: text("An update is ready.") });
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(ondismiss).toHaveBeenCalledOnce();
  });

  it("has no dismiss button otherwise, and shows its actions", () => {
    render(Banner, {
      children: text("Saved."),
      actions: htmlSnippet("<button>Restart atlas</button>"),
    });
    expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();
    expect(screen.getByRole("button", { name: "Restart atlas" })).toBeInTheDocument();
  });

  it("shows a spinner in place of its icon while busy", () => {
    const { container } = render(Banner, { busy: true, children: text("Starting atlas…") });
    expect(container.querySelector(".ui-spinner")).not.toBeNull();
    expect(container.querySelector("svg")).toBeNull();
  });
});

describe("Kbd", () => {
  it("writes Mod as Ctrl off Apple devices", () => {
    const { container } = render(Kbd, { keys: ["Mod", "K"], platform: "other" });
    expect(container.querySelectorAll(".ui-kbd > kbd")).toHaveLength(2);
    expect(container).toHaveTextContent("Ctrl");
  });

  it("draws Apple modifiers as symbols, with their names for assistive technology", () => {
    const { container } = render(Kbd, { keys: ["Mod", "Shift", "P"], platform: "apple" });
    const symbols = [...container.querySelectorAll('[aria-hidden="true"]')].map(
      (node) => node.textContent,
    );
    expect(symbols).toEqual(["⌘", "⇧"]);
    expect(screen.getByText("Command")).toBeInTheDocument();
    expect(screen.getByText("Shift")).toBeInTheDocument();
  });
});
