export function numberOfText(text: string | number): number | null {
  if (typeof text === "number") return Number.isFinite(text) ? text : null;
  const trimmed = text.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

export function textOfNumber(value: number | null): string {
  return value === null ? "" : String(value);
}
