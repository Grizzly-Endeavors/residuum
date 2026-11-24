import { fieldRefKey, type FieldRef } from "../../lib/settings-fields";

const FOCUSABLE =
  "input:not([type='hidden']):not(:disabled), select:not(:disabled), textarea:not(:disabled), button:not(:disabled)";

export function fieldMark(ref: FieldRef): string {
  return fieldRefKey(ref);
}

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
