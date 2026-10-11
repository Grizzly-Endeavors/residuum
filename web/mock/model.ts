import { artifactIdentity, json, readJsonObject, type JsonObject } from "./http";
import type { Route, RouteContext } from "./routes";

/**
 * Model calls are slowed down so an artifact's calls stay in flight for a
 * moment, long enough to see a burst of them queue in the SDK's model lane.
 */
const MODEL_CALL_DELAY_MS = 3000;

/** The name of the header a workbench artifact's requests carry. */
const ARTIFACT_HEADER = "x-residuum-artifact";

/** What `POST /api/model/complete` answers. */
interface ModelCompleteResponse {
  content: string;
  json?: unknown;
  model: string;
  usage: { input_tokens: number; output_tokens: number };
}

/** The text the call is about: the prompt, or the last message of the conversation. */
type Request = { kind: "ok"; prompt: string } | { kind: "invalid"; status: number; error: string };

function invalid(status: number, error: string): Request {
  return { kind: "invalid", status, error };
}

/** Read a request body the way the backend's `build_request` does: either a `prompt`, or `messages`, which win. */
function readRequest(body: JsonObject): Request {
  const { messages, prompt } = body;
  if (messages === undefined || messages === null) {
    if (typeof prompt !== "string" || prompt.trim() === "") {
      return invalid(400, 'request needs a non-empty "prompt" or "messages"');
    }
    return { kind: "ok", prompt };
  }
  if (!Array.isArray(messages)) {
    return invalid(422, 'Failed to deserialize the JSON body: "messages" must be a list');
  }
  if (messages.length === 0) return invalid(400, '"messages" must not be empty');
  let last = "";
  for (const message of messages as unknown[]) {
    const { role, content } = (message ?? {}) as JsonObject;
    if (typeof role !== "string" || typeof content !== "string") {
      return invalid(
        422,
        "Failed to deserialize the JSON body: a message needs a role and content",
      );
    }
    if (role !== "user" && role !== "assistant") {
      return invalid(400, `message role must be "user" or "assistant", got "${role}"`);
    }
    if (content.trim() === "") return invalid(400, "message content must not be empty");
    last = content;
  }
  return { kind: "ok", prompt: last };
}

/** A value of the shape `schema` describes, for a call that asked for structured output. */
function sampleFromSchema(schema: unknown): unknown {
  if (typeof schema !== "object" || schema === null) return null;
  const { type, properties, enum: choices } = schema as JsonObject;
  if (Array.isArray(choices)) return (choices as unknown[])[0] ?? null;
  switch (type) {
    case "object":
      return Object.fromEntries(
        Object.entries((properties ?? {}) as JsonObject).map(([key, sub]) => [
          key,
          sampleFromSchema(sub),
        ]),
      );
    case "array":
      return [];
    case "string":
      return "mock";
    case "number":
    case "integer":
      return 0;
    case "boolean":
      return false;
    default:
      return null;
  }
}

/**
 * `POST /api/model/complete`: a one-shot model call on an artifact's behalf.
 * Malformed requests are refused the way the backend refuses them, at once;
 * a good one is answered after `delayMs`.
 */
function completeHandler(delayMs: number): (ctx: RouteContext) => Promise<void> {
  return async ({ req, res, state }) => {
    if (req.headers[ARTIFACT_HEADER] !== undefined && artifactIdentity(req) === null) {
      json(res, 400, {
        error: `the ${ARTIFACT_HEADER} header must name an artifact, like "wiki-graph"`,
      });
      return;
    }
    const body = await readJsonObject(req);
    const request = readRequest(body);
    if (request.kind === "invalid") {
      json(res, request.status, { error: request.error });
      return;
    }
    const sample =
      body.schema === undefined || body.schema === null ? undefined : sampleFromSchema(body.schema);
    const content =
      sample === undefined
        ? `Mock model reply to: ${request.prompt.slice(0, 200)}`
        : JSON.stringify(sample);
    // A brief artificial delay, so a call stays in flight long enough to see.
    await state.env.sleep(delayMs);
    json(res, 200, {
      content,
      ...(sample === undefined ? {} : { json: sample }),
      model: "mock/small",
      usage: { input_tokens: request.prompt.length, output_tokens: content.length },
    } satisfies ModelCompleteResponse);
  };
}

/** The model call route, answering after `delayMs` of simulated time (see `MockEnv.after`). */
export function createModelRoutes(delayMs = MODEL_CALL_DELAY_MS): readonly Route[] {
  return [{ method: "POST", pattern: "/api/model/complete", handler: completeHandler(delayMs) }];
}

/** The model call route, with the delay that shows a call in flight. */
export const modelRoutes: readonly Route[] = createModelRoutes();
