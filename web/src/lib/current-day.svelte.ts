// The present moment, for what is named by how far from today it is
// ("Today", "Yesterday"). Reading it inside an effect or a derived value
// makes that re-run every minute for as long as something reads it, so a page
// left open across midnight renames its days.

import { createSubscriber } from "svelte/reactivity";

const MINUTE_MS = 60_000;

const subscribe = createSubscriber((update) => {
  const timer = window.setInterval(update, MINUTE_MS);
  return () => {
    window.clearInterval(timer);
  };
});

/** The current time in milliseconds, reactive to the passing of time. */
export function currentMoment(): number {
  subscribe();
  return Date.now();
}
