/**
 * How long a test, a hook, or one of its waits runs before it is taken to have
 * hung. Every wait ends on the condition it checks, so this bound only matters
 * when something never happens; it sits well above how long a loaded machine
 * takes to render and settle, which the per-call defaults (1s for a wait, 5s
 * for a test) did not. Kept free of imports so the Vite config can read it.
 */
export const HANG_GUARD_MS = 30_000;

/** How long `findBy*` and a wait inside a test look before giving up. */
export const QUERY_GUARD_MS = 20_000;
