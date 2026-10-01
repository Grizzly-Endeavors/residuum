import type { Attachment } from "svelte/attachments";

/**
 * Makes a scrolling region a tab stop while its content overflows, so the
 * keyboard can reach it and scroll it. Chrome and Firefox already let the
 * keyboard into any scroller; Safari doesn't, which is what axe's
 * scrollable-region-focusable checks. A region with nothing to scroll stays
 * out of the tab order, and one that has focus keeps its stop until focus
 * leaves. Give the element a role and an accessible name, so the stop says
 * what it holds.
 */
export const keyboardScrollable: Attachment<HTMLElement> = (element) => {
  let frame = 0;

  function update(): void {
    frame = 0;
    const overflows =
      element.scrollHeight > element.clientHeight || element.scrollWidth > element.clientWidth;
    if (overflows) element.tabIndex = 0;
    else if (document.activeElement !== element) element.removeAttribute("tabindex");
  }

  function schedule(): void {
    if (frame === 0) frame = requestAnimationFrame(update);
  }

  // The region's size, and its children's: content can grow without a DOM
  // change, as an image does when it loads.
  const resizes = new ResizeObserver(schedule);
  resizes.observe(element);
  for (const child of element.children) resizes.observe(child);

  const mutations = new MutationObserver((records) => {
    for (const record of records) {
      if (record.target !== element) continue;
      for (const node of record.removedNodes) if (node instanceof Element) resizes.unobserve(node);
      for (const node of record.addedNodes) if (node instanceof Element) resizes.observe(node);
    }
    schedule();
  });
  mutations.observe(element, { childList: true, subtree: true, characterData: true });
  element.addEventListener("blur", schedule);
  update();

  return () => {
    cancelAnimationFrame(frame);
    resizes.disconnect();
    mutations.disconnect();
    element.removeEventListener("blur", schedule);
  };
};
