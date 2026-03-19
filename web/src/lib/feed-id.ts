// Single source of truth for feed item IDs across all modules.

let counter = 0;

/** Generate a unique, monotonically increasing feed item ID. */
export function nextFeedId(): number {
  return ++counter;
}
