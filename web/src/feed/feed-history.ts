/** Older history a feed loads as the reader nears the top. */
export interface FeedHistory {
  /** More remains to load. */
  readonly hasMore: boolean;
  readonly loadingOlder: boolean;
  /** Bumped whenever the whole feed is replaced, so a scrolled-up reader is put back in place. */
  readonly generation: number;
  /** Load the next older part; resolves whether anything was added. */
  loadOlder: () => Promise<boolean>;
}
