import type { IncomingMessage, ServerResponse } from "node:http";
import { isValidArtifactName } from "./artifact-name";

/** A parsed JSON request body: an object whose fields still have to be checked. */
export type JsonObject = Record<string, unknown>;

export function readBody(req: IncomingMessage): Promise<string> {
  return new Promise<string>((resolve) => {
    let data = "";
    req.on("data", (chunk: Buffer) => {
      data += chunk.toString();
    });
    req.on("end", () => {
      resolve(data);
    });
  });
}

/** Parse a request body that must be a JSON object; anything else throws. */
export function parseJsonObject(raw: string): JsonObject {
  const value: unknown = JSON.parse(raw);
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("the request body must be a JSON object");
  }
  return value as JsonObject;
}

export async function readJsonObject(req: IncomingMessage): Promise<JsonObject> {
  return parseJsonObject(await readBody(req));
}

/** The field's value when it is a string, `undefined` otherwise. */
export function stringField(body: JsonObject, key: string): string | undefined {
  const value = body[key];
  return typeof value === "string" ? value : undefined;
}

export function json(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

export function text(res: ServerResponse, status: number, body: string): void {
  res.writeHead(status, { "Content-Type": "text/plain" });
  res.end(body);
}

/** Whether the request carries an `X-Residuum-Artifact` header, valid or not. */
export function hasArtifactHeader(req: IncomingMessage): boolean {
  return req.headers["x-residuum-artifact"] !== undefined;
}

/** The `X-Residuum-Artifact` header, or `null` when it's absent or isn't an artifact's name. */
export function artifactIdentity(req: IncomingMessage): string | null {
  const raw = req.headers["x-residuum-artifact"];
  const value = Array.isArray(raw) ? raw[0] : raw;
  return value !== undefined && isValidArtifactName(value) ? value : null;
}
