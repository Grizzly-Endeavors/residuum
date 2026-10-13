// Notices for an address the backend sent a browser to. When someone opens a
// link the backend can't serve (a file link that expired, an artifact that was
// renamed, a webhook address), it redirects them into the app with a code in
// the `notice` query parameter (`src/gateway/browser_landing.rs`), and the
// app tells them what happened where they can act on it. The words live here
// and the backend sends only codes, so a crafted link can't put its own text
// in front of the user. The router drops the parameter once it's read.

import { isArtifactName, viewedAgentOf, type AppLocation } from "./routes";

/** The query parameter carrying the code. */
export const NOTICE_PARAM = "notice";

/** The query parameter naming the missing artifact, beside `artifact-missing`. */
const ARTIFACT_PARAM = "artifact";

/** What a notice may name. */
interface NoticeContext {
  /** The agent whose place the notice shows on, if any. */
  agent: string | null;
  /** The artifact the address named, if it could be one. */
  artifact: string | null;
}

const MESSAGES: Readonly<Record<string, (context: NoticeContext) => string>> = {
  "artifact-missing": ({ artifact }) =>
    artifact === null
      ? "There's no workbench artifact at that address. It may have been renamed or deleted."
      : `There's no workbench artifact named "${artifact}". It may have been renamed or deleted.`,
  "artifact-file-missing": () =>
    "That artifact has no file at the address you opened. It may have been moved or removed.",
  "file-link-expired": ({ agent }) =>
    `That file link has expired. Ask ${agent ?? "the agent"} to send the file again.`,
  "agent-stopped": ({ agent }) =>
    `That link needs ${agent ?? "its agent"} running. Start ${agent ?? "it"}, then open the link again.`,
  "inbox-attachment-missing": () =>
    "That attachment is no longer available. It may have been removed from its inbox item.",
  "webhook-address": () =>
    "That address is a webhook: other services send POST requests to it to give an agent work, so there's nothing to open in a browser. Its settings are under Connections.",
};

/**
 * The notices `search` (a URL's query) carries for `location` (where it
 * leads). An unknown code is ignored, since it can only come from a link
 * someone wrote by hand.
 */
export function landingNotices(search: string, location: AppLocation): string[] {
  const query = new URLSearchParams(search);
  const artifact = query.get(ARTIFACT_PARAM);
  const context: NoticeContext = {
    agent: viewedAgentOf(location.place),
    artifact: artifact !== null && isArtifactName(artifact) ? artifact : null,
  };
  const notices: string[] = [];
  for (const code of query.getAll(NOTICE_PARAM)) {
    const message = Object.hasOwn(MESSAGES, code) ? MESSAGES[code] : undefined;
    if (message !== undefined && !notices.includes(message(context))) {
      notices.push(message(context));
    }
  }
  return notices;
}
