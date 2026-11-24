const THIS_YEAR = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const OTHER_YEAR = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  year: "numeric",
});

const DAY_PART = /^(\d{4})-(\d{2})-(\d{2})/;
const MS_PER_DAY = 86_400_000;

function dayOf(value: string): Date | null {
  const match = DAY_PART.exec(value);
  if (match === null) return null;
  const day = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  return Number.isNaN(day.getTime()) ? null : day;
}

function daysBetween(from: Date, to: Date): number {
  const utc = (day: Date): number => Date.UTC(day.getFullYear(), day.getMonth(), day.getDate());
  return Math.round((utc(to) - utc(from)) / MS_PER_DAY);
}

export function dayLabel(value: string, now: number = Date.now()): string {
  const day = dayOf(value);
  if (day === null) return value.slice(0, 10);
  const today = new Date(now);
  const ago = daysBetween(day, today);
  if (ago === 0) return "Today";
  if (ago === 1) return "Yesterday";
  return (day.getFullYear() === today.getFullYear() ? THIS_YEAR : OTHER_YEAR).format(day);
}
