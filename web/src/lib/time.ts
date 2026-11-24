export function relativeTime(then: Date | string, now: number = Date.now()): string {
  const at = typeof then === "string" ? new Date(then) : then;
  const seconds = Math.max(0, Math.round((now - at.getTime()) / 1000));
  if (Number.isNaN(seconds)) return "";
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

export function isoNow(): string {
  return new Date().toISOString();
}

// ── Timestamps on the reader's clock ─────────────────────────────────
//
// Chat history stamps each message with a local date and time and no zone
// ("2026-10-09T23:30:12"), which a browser reads as its own. The messages the
// page stamps itself take the same shape, so a day boundary falls in the same
// place for both: an RFC 3339 timestamp in UTC would put a message sent at
// 23:30 local time on the next day's date in the Americas, and one sent at
// 00:30 on the previous day's in Asia and Oceania.

const ZONE_SUFFIX = /(?:Z|[+-]\d{2}:?\d{2})$/i;

/** More than three fractional digits, which not every browser reads. */
const LONG_FRACTION = /(\.\d{3})\d+/;

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

function dateOf(at: Date): string {
  return `${String(at.getFullYear())}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
}

export function localTimestamp(at: Date = new Date()): string {
  return `${dateOf(at)}T${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`;
}

export function localTimestampAt(ms: number): string {
  return localTimestamp(new Date(ms));
}

export function parseTimestamp(timestamp: string): Date | null {
  const at = new Date(timestamp.replace(LONG_FRACTION, "$1"));
  return Number.isNaN(at.getTime()) ? null : at;
}

export function localDay(timestamp: string): string {
  if (ZONE_SUFFIX.test(timestamp)) {
    const at = parseTimestamp(timestamp);
    if (at !== null) return dateOf(at);
  }
  return timestamp.slice(0, 10);
}

const CLOCK: Intl.DateTimeFormatOptions = { hour: "numeric", minute: "2-digit" };
const DAY_AND_CLOCK: Intl.DateTimeFormatOptions = { month: "short", day: "numeric", ...CLOCK };
const YEAR_DAY_AND_CLOCK: Intl.DateTimeFormatOptions = { year: "numeric", ...DAY_AND_CLOCK };

export function messageTimeLabel(timestamp: string, now: number = Date.now()): string {
  const at = parseTimestamp(timestamp);
  if (at === null) return "";
  const today = new Date(now);
  const format = (options: Intl.DateTimeFormatOptions): string =>
    new Intl.DateTimeFormat(undefined, options).format(at);
  if (dateOf(at) === dateOf(today)) return format(CLOCK);
  return format(at.getFullYear() === today.getFullYear() ? DAY_AND_CLOCK : YEAR_DAY_AND_CLOCK);
}
