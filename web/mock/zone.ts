/**
 * The mock hub's timezone, and the conversions the hub makes between instants
 * and the naive local times its agents' files keep (`src/time.rs`).
 */

/** The hub timezone the mock reports from `GET /api/system/timezone` and reads its schedules in. */
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

/** The wall clock of the hub's zone at `instantMs`, as a millisecond count read as if it were UTC. */
export function localMs(instantMs: number): number {
  const whole = Math.floor(instantMs / SECOND_MS) * SECOND_MS;
  const field = new Map<string, number>();
  for (const part of FORMAT.formatToParts(new Date(whole))) {
    field.set(part.type, Number(part.value));
  }
  const at = (type: string): number => field.get(type) ?? 0;
  return Date.UTC(at("year"), at("month") - 1, at("day"), at("hour"), at("minute"), at("second"));
}

/** How far the hub's zone is ahead of UTC at `instantMs`, in milliseconds. */
function offsetMs(instantMs: number): number {
  return localMs(instantMs) - Math.floor(instantMs / SECOND_MS) * SECOND_MS;
}

/**
 * The instant a wall-clock time in the hub's zone denotes, as `local_to_instant`
 * resolves it: the earlier of two instants when a fall-back repeats the time,
 * and, for a time a spring-forward skips, the offset in force before the gap.
 */
export function instantOfLocal(local: number): number {
  const candidates = [offsetMs(local - DAY_MS), offsetMs(local + DAY_MS)]
    .map((offset) => local - offset)
    .filter((instant) => localMs(instant) === local);
  if (candidates.length > 0) return Math.min(...candidates);
  return local - offsetMs(local - DAY_MS);
}

/** `instantMs` as RFC 3339 with whole seconds and the hub zone's offset, `Z` when it is zero. */
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
