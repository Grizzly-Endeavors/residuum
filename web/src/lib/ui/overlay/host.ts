/**
 * Where overlay layers render, and what counts as focusable inside one.
 *
 * Every layer moves into one host, a direct child of `<body>`. No ancestor can
 * clip it, trap it in a stacking context or turn `position: fixed` into
 * something else, and a modal can make the rest of the page inert by marking
 * the host's siblings.
 */

import type { Attachment } from "svelte/attachments";

let host: HTMLElement | null = null;

/** The overlay host, created on first use. */
export function overlayHost(): HTMLElement {
  if (host?.isConnected) return host;
  host = document.createElement("div");
  host.setAttribute("data-overlay-host", "");
  // Its layers position themselves against the viewport; the host adds no box of its own.
  host.style.display = "contents";
  document.body.append(host);
  return host;
}

/**
 * Moves the element into the overlay host. Svelte removes a block by walking
 * its nodes from first to last, so the moved element must never be the first
 * or last node of a block: render it between two empty `<template>` elements.
 */
export const portal: Attachment<HTMLElement> = (element) => {
  overlayHost().append(element);
  return () => {
    element.remove();
  };
};

const FOCUSABLE = [
  "a[href]",
  "button",
  "input:not([type='hidden'])",
  "select",
  "textarea",
  "summary",
  "iframe",
  "[contenteditable]:not([contenteditable='false'])",
  "[tabindex]",
].join(",");

/** What Tab reaches inside `root`, in order. */
export function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (element) =>
      element.tabIndex >= 0 &&
      !element.matches(":disabled") &&
      element.closest("[hidden], [inert]") === null &&
      // Layout-aware where the browser has it; jsdom doesn't.
      (typeof element.checkVisibility !== "function" || element.checkVisibility()),
  );
}

/**
 * Focus what a layer starts on: `selector`'s match (`[data-autofocus]` by
 * default), else the first focusable that isn't a close button, else
 * `container` itself. `false` goes straight to the container.
 */
export function focusInitial(container: HTMLElement, selector?: string | false): void {
  let target: HTMLElement | null | undefined = null;
  if (selector !== false) {
    target =
      container.querySelector<HTMLElement>(selector ?? "[data-autofocus]") ??
      focusables(container).find((element) => !element.hasAttribute("data-overlay-close"));
  }
  (target ?? container).focus();
}
