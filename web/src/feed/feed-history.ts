export interface FeedHistory {
  readonly hasMore: boolean;
  readonly loadingOlder: boolean;
  readonly generation: number;
  loadOlder: () => Promise<boolean>;
}
