// Focusing a settings field on arrival. A section marks each field it holds
// with `data-field={fieldMark(ref)}` on the field or an element around it,
// and focuses the one a scope's `focusRequest` names once it shows: the field
// Fix settings found, or the one another section's button pointed at.

import { fieldRefKey, type FieldRef } from "../../lib/settings-fields";

const FOCUSABLE =
  "input:not([type='hidden']):not(:disabled), select:not(:disabled), textarea:not(:disabled), button:not(:disabled)";

/** The `data-field` value that marks a field for `focusField`. */
export function fieldMark(ref: FieldRef): string {
  return fieldRefKey(ref);
}

/**
 * Focus the field `ref` names under `root`, which also scrolls it into view: a
 * control marked invalid first, else the first control. The field carries
 * `data-arrived`, which a section draws as a highlight, until focus leaves it.
 * False when no marked field is there to focus.
 */
export function focusField(root: ParentNode, ref: FieldRef): boolean {
  const mark = fieldMark(ref);
  const holder = [...root.querySelectorAll<HTMLElement>("[data-field]")].find(
    (element) => element.dataset.field === mark,
  );
  if (holder === undefined) return false;
  const target =
    holder.querySelector<HTMLElement>("[aria-invalid='true']") ??
    (holder.matches(FOCUSABLE) ? holder : holder.querySelector<HTMLElement>(FOCUSABLE));
  if (target === null) return false;
  holder.setAttribute("data-arrived", "");
  target.focus();
  holder.addEventListener(
    "focusout",
    () => {
      holder.removeAttribute("data-arrived");
    },
    { once: true },
  );
  return true;
}
