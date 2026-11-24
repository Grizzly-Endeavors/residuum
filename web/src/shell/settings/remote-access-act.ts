export type RemoteAct = (name: string, failure: string, step: () => Promise<void>) => Promise<void>;
