import { artifactIdentity, json, readJsonObject, stringField, text } from "./http";
import type { Route, RouteContext } from "./routes";
import type { MockState } from "./state";

/** The name of the header a workbench artifact's requests carry. */
const ARTIFACT_HEADER = "x-residuum-artifact";

/**
 * An item's id, as the backend makes it: the day and the title's words, with
 * `_2`, `_3` and so on when that day already has an item of the same title.
 */
function inboxItemId(state: MockState, title: string): string {
  const day = state.env.clock.iso().slice(0, 10).replaceAll("-", "");
  const slug = title
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter((word) => word !== "")
    .join("_")
    .slice(0, 60)
    .replace(/_+$/, "");
  const stem = slug === "" ? day : `${day}_${slug}`;
  let id = stem;
  for (let suffix = 2; state.agentInbox.some((item) => item.id === id); suffix++) {
    id = `${stem}_${String(suffix)}`;
  }
  return id;
}

/**
 * `POST .../agent-inbox`: add an item to the agent's own inbox, for it to
 * triage later. The body is `{ title?, body }`, and the title defaults to the
 * body's first line. The item's source is the artifact the request names, or
 * `web`. Answers `{ id }`.
 */
async function addAgentInboxItem({ req, res, state }: RouteContext): Promise<void> {
  let body: Awaited<ReturnType<typeof readJsonObject>>;
  try {
    body = await readJsonObject(req);
  } catch (err) {
    const why = err instanceof Error ? err.message : String(err);
    text(res, 400, `Failed to parse the request body as JSON: ${why}`);
    return;
  }
  const content = stringField(body, "body");
  if (content === undefined) {
    text(res, 422, "Failed to deserialize the JSON body: missing string field `body`");
    return;
  }
  if (content.trim() === "") {
    text(res, 400, "body must not be blank");
    return;
  }
  const artifact = artifactIdentity(req);
  if (req.headers[ARTIFACT_HEADER] !== undefined && artifact === null) {
    text(res, 400, `the ${ARTIFACT_HEADER} header must name an artifact, like "wiki-graph"`);
    return;
  }
  const given = stringField(body, "title");
  const title = given !== undefined && given.trim() !== "" ? given : (content.split("\n")[0] ?? "");
  const id = inboxItemId(state, title);
  state.agentInbox.push({
    id,
    title,
    body: content,
    source: artifact === null ? "web" : `artifact:${artifact}`,
    timestamp: state.env.clock.iso(),
  });
  json(res, 200, { id });
}

/** The agent inbox route, in the unscoped `/api/...` spelling. It is one of the routes only a running agent serves. */
export const agentInboxRoutes: readonly Route[] = [
  { method: "POST", pattern: "/api/agent-inbox", handler: addAgentInboxItem },
];
