// A calendar day as the feed's dividers name it: "Today", "Yesterday", then
// "Oct 6", with the year once it isn't this one.

const THIS_YEAR = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const OTHER_YEAR = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  year: "numeric",
});

const DAY_PART = /^(\d{4})-(\d{2})-(\d{2})/;
const MS_PER_DAY = 86_400_000;

/**
 * The day a "YYYY-MM-DD" date or a "YYYY-MM-DDTHH:MM" timestamp falls on, as
 * a local date at midnight, or null when it names no day.
 */
function dayOf(value: string): Date | null {
  const match = DAY_PART.exec(value);
  if (match === null) return null;
  const day = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  return Number.isNaN(day.getTime()) ? null : day;
}

/** Whole calendar days from `from` to `to`, unaffected by clock changes. */
function daysBetween(from: Date, to: Date): number {
  const utc = (day: Date): number => Date.UTC(day.getFullYear(), day.getMonth(), day.getDate());
  return Math.round((utc(to) - utc(from)) / MS_PER_DAY);
}

/**
 * The name of the day `value` falls on, seen from `now`, in epoch milliseconds. A value that names
 * no day comes back as its first ten characters.
 */
export function dayLabel(value: string, now: number = Date.now()): string {
  const day = dayOf(value);
  if (day === null) return value.slice(0, 10);
  const today = new Date(now);
  const ago = daysBetween(day, today);
  if (ago === 0) return "Today";
  if (ago === 1) return "Yesterday";
  return (day.getFullYear() === today.getFullYear() ? THIS_YEAR : OTHER_YEAR).format(day);
}
