/**
 * Focus an element as soon as it mounts. Svelte's own `autofocus` attribute
 * triggers an accessibility lint warning (it steals focus regardless of how
 * the element appeared); this action does the same thing for an element
 * that appears through explicit user action — e.g. clicking "Rename" — where
 * that's the expected, helpful behavior.
 */
export function focusOnMount(node: HTMLElement): void {
  node.focus();
}
