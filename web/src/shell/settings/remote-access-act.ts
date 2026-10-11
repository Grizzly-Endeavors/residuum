/** Runs one action of the Remote access group: marks `name` busy, reports `failure` if it throws, then reads the status again. */
export type RemoteAct = (name: string, failure: string, step: () => Promise<void>) => Promise<void>;
