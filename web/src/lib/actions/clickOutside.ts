export function clickOutside(
  node: HTMLElement,
  params: { onOutside: () => void },
): { update(params: { onOutside: () => void }): void; destroy(): void } {
  let { onOutside } = params;

  function handle(e: MouseEvent): void {
    if (!node.contains(e.target as Node)) {
      onOutside();
    }
  }

  document.addEventListener("mousedown", handle);

  return {
    update(p) {
      onOutside = p.onOutside;
    },
    destroy() {
      document.removeEventListener("mousedown", handle);
    },
  };
}
