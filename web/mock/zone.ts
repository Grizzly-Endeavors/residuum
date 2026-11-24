export const MOCK_TIMEZONE = "America/New_York";

const SECOND_MS = 1000;
const DAY_MS = 86_400_000;

const FORMAT = new Intl.DateTimeFormat("en-US", {
  timeZone: MOCK_TIMEZONE,
  hourCycle: "h23",
  year: "numeric",
  month: "numeric",
  day: "numeric",
  hour: "numeric",
  minute: "numeric",
  second: "numeric",
});

export function localMs(instantMs: number): number {
  const whole = Math.floor(instantMs / SECOND_MS) * SECOND_MS;
  const field = new Map<string, number>();
  for (const part of FORMAT.formatToParts(new Date(whole))) {
    field.set(part.type, Number(part.value));
  }
  const at = (type: string): number => field.get(type) ?? 0;
  return Date.UTC(at("year"), at("month") - 1, at("day"), at("hour"), at("minute"), at("second"));
}

function offsetMs(instantMs: number): number {
  return localMs(instantMs) - Math.floor(instantMs / SECOND_MS) * SECOND_MS;
}

export function instantOfLocal(local: number): number {
  const candidates = [offsetMs(local - DAY_MS), offsetMs(local + DAY_MS)]
    .map((offset) => local - offset)
    .filter((instant) => localMs(instant) === local);
  if (candidates.length > 0) return Math.min(...candidates);
  return local - offsetMs(local - DAY_MS);
}

export function rfc3339InZone(instantMs: number): string {
  const offset = offsetMs(instantMs);
  const wall = new Date(localMs(instantMs)).toISOString().slice(0, 19);
  if (offset === 0) return `${wall}Z`;
  const minutes = Math.abs(offset) / 60_000;
  const sign = offset < 0 ? "-" : "+";
  const hh = String(Math.floor(minutes / 60)).padStart(2, "0");
  const mm = String(minutes % 60).padStart(2, "0");
  return `${wall}${sign}${hh}:${mm}`;
}
