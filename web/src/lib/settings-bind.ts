// Settings forms keep every number as text (`ConfigFields`), so an empty box
// means "not set" and a save leaves the key out. A `NumberField` binds to a
// number or null; these two convert between the form's text and the box.

/** The number a form field's text holds, or null while the box is empty or isn't a number. A number input may leave a number in the field. */
export function numberOfText(text: string | number): number | null {
  if (typeof text === "number") return Number.isFinite(text) ? text : null;
  const trimmed = text.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

/** The text a form field keeps for the box's number: empty when the box is. */
export function textOfNumber(value: number | null): string {
  return value === null ? "" : String(value);
}
