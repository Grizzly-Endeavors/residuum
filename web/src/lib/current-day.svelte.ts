import { createSubscriber } from "svelte/reactivity";

const MINUTE_MS = 60_000;

const subscribe = createSubscriber((update) => {
  const timer = window.setInterval(update, MINUTE_MS);
  return () => {
    window.clearInterval(timer);
  };
});

export function currentMoment(): number {
  subscribe();
  return Date.now();
}
