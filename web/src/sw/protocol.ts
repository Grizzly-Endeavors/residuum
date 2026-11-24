export const WORKER_URL = "/sw.js";

export const SKIP_WAITING_MESSAGE = { type: "skip-waiting" } as const;

export type PageMessage = typeof SKIP_WAITING_MESSAGE;

export function isPageMessage(data: unknown): data is PageMessage {
  return (
    typeof data === "object" &&
    data !== null &&
    "type" in data &&
    data.type === SKIP_WAITING_MESSAGE.type
  );
}

export interface OpenTargetMessage {
  type: "open-target";
  target: string;
}

export interface EndpointChangedMessage {
  type: "endpoint-changed";
  endpoint: string;
}

export type WorkerMessage = OpenTargetMessage | EndpointChangedMessage;

export function openTargetMessage(target: string): OpenTargetMessage {
  return { type: "open-target", target };
}

export function endpointChangedMessage(endpoint: string): EndpointChangedMessage {
  return { type: "endpoint-changed", endpoint };
}

export function isWorkerMessage(data: unknown): data is WorkerMessage {
  if (typeof data !== "object" || data === null || !("type" in data)) return false;
  if (data.type === "open-target") return "target" in data && typeof data.target === "string";
  if (data.type === "endpoint-changed") {
    return "endpoint" in data && typeof data.endpoint === "string";
  }
  return false;
}
