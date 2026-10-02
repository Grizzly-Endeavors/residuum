import type { Choice, ChoiceGroup } from "./ui/types";

/** Time zone names for a dropdown: UTC first, then one group per area. */
export interface TimeZoneChoices {
  readonly ungrouped: readonly Choice[];
  readonly groups: readonly ChoiceGroup[];
}

/**
 * Names residuum accepts that `Intl.supportedValuesOf("timeZone")` leaves out
 * on some runtimes. `UTC` is one of those, and it is a normal choice here.
 */
const PINNED = ["UTC"] as const;

/** Whether `name` is an IANA time zone this runtime accepts. */
export function isTimeZoneName(name: string): boolean {
  try {
    Intl.DateTimeFormat(undefined, { timeZone: name });
    return true;
  } catch {
    return false;
  }
}

/**
 * Choices for the time zone dropdown.
 *
 * A `current` value the runtime doesn't list (a name saved by hand that isn't
 * a real zone) is included so the control can still show it.
 */
export function timeZoneChoices(current: string): TimeZoneChoices {
  const names = Intl.supportedValuesOf("timeZone");
  const known = new Set<string>(names);
  const ungrouped: Choice[] = PINNED.map((name) => ({ value: name, label: name }));
  const pinned = new Set<string>(PINNED);
  const byArea = new Map<string, Choice[]>();
  for (const name of names) {
    if (pinned.has(name)) continue;
    const slash = name.indexOf("/");
    if (slash === -1) {
      ungrouped.push({ value: name, label: name });
      continue;
    }
    const area = name.slice(0, slash);
    const list = byArea.get(area) ?? [];
    list.push({ value: name, label: name });
    byArea.set(area, list);
  }
  const trimmed = current.trim();
  if (trimmed !== "" && !known.has(trimmed) && !pinned.has(trimmed)) {
    ungrouped.unshift({ value: trimmed, label: trimmed });
  }
  const groups = [...byArea.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([label, options]) => ({ label, options }));
  return { ungrouped, groups };
}
