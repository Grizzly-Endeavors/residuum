import type { Attachment } from "svelte/attachments";

let host: HTMLElement | null = null;

export function overlayHost(): HTMLElement {
  if (host?.isConnected) return host;
  host = document.createElement("div");
  host.setAttribute("data-overlay-host", "");
  host.style.display = "contents";
  document.body.append(host);
  return host;
}

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

export function focusInitial(container: HTMLElement, selector?: string | false): void {
  let target: HTMLElement | null | undefined = null;
  if (selector !== false) {
    target =
      container.querySelector<HTMLElement>(selector ?? "[data-autofocus]") ??
      focusables(container).find((element) => !element.hasAttribute("data-overlay-close"));
  }
  (target ?? container).focus();
}
