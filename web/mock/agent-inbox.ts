import { artifactIdentity, json, readJsonObject, stringField, text } from "./http";
import type { Route, RouteContext } from "./routes";
import type { MockState } from "./state";

const ARTIFACT_HEADER = "x-residuum-artifact";

export function agentInboxItemId(states: readonly MockState[], title: string): string {
  const [first] = states;
  if (first === undefined) throw new Error("an item id needs the inbox it goes in");
  const day = first.env.clock.iso().slice(0, 10).replaceAll("-", "");
  const slug = title
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter((word) => word !== "")
    .join("_")
    .slice(0, 60)
    .replace(/_+$/, "");
  const stem = slug === "" ? day : `${day}_${slug}`;
  let id = stem;
  const taken = (candidate: string): boolean =>
    states.some((each) => each.agentInbox.some((item) => item.id === candidate));
  for (let suffix = 2; taken(id); suffix++) {
    id = `${stem}_${String(suffix)}`;
  }
  return id;
}

export function deriveTitle(body: string): string {
  return body === "" ? "Inbox message" : (body.split(/\r?\n/)[0] ?? "");
}

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
  const title = given !== undefined && given.trim() !== "" ? given : deriveTitle(content);
  const id = agentInboxItemId([state], title);
  state.agentInbox.push({
    id,
    title,
    body: content,
    source: artifact === null ? "web" : `artifact:${artifact}`,
    timestamp: state.env.clock.iso(),
  });
  json(res, 200, { id });
}

export const agentInboxRoutes: readonly Route[] = [
  { method: "POST", pattern: "/api/agent-inbox", handler: addAgentInboxItem },
];
