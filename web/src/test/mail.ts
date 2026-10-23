// Threads, messages and a fake hub for the mail view tests, and a reset for
// the mail store singleton the views share.

import { vi } from "vitest";
import type { MailMessage, MailThread, MailThreadSummary } from "../lib/hub-types";
import { mail } from "../lib/mail.svelte";
import { jsonResponse, mockFetch } from "./component";

/** A list row: a read one-message thread in the inbox, with the first agent of `id` in it. */
export function mailSummary(
  id: string,
  overrides: Partial<MailThreadSummary> = {},
): MailThreadSummary {
  return {
    thread_id: id,
    subject: `Subject of ${id}`,
    participants: [id.split("/")[0] ?? "atlas"],
    message_count: 1,
    latest_at: "2026-03-14T11:00:00Z",
    snippet: `Snippet of ${id}`,
    unread: false,
    has_attachments: false,
    in_inbox: true,
    in_sent: false,
    archived: false,
    ...overrides,
  };
}

export function agentMessage(ref: string, overrides: Partial<MailMessage> = {}): MailMessage {
  return {
    ref,
    from: { kind: "agent", agent: ref.split("/")[0] ?? "atlas" },
    to: [],
    at: "2026-03-14T12:00:00Z",
    title: ref,
    body: "Hello",
    attachments: [],
    in_reply_to: null,
    read: true,
    archived: false,
    seen_by: [],
    ...overrides,
  };
}

export function userMessage(ref: string, overrides: Partial<MailMessage> = {}): MailMessage {
  return {
    ...agentMessage(ref),
    from: { kind: "user" },
    to: ["atlas"],
    seen_by: [{ agent: "atlas", seen: false }],
    ...overrides,
  };
}

export function mailThread(
  id: string,
  messages: MailMessage[],
  overrides: Partial<MailThread> = {},
): MailThread {
  return {
    thread_id: id,
    subject: "Wiki audit",
    participants: ["atlas", "scout"],
    messages,
    recipients_for_reply: ["atlas", "scout"],
    ...overrides,
  };
}

/** A request the fake hub saw: its method, path (decoded) and body. */
export interface SeenRequest {
  method: string;
  path: string;
  body: unknown;
}

export type MailRoute = (request: SeenRequest) => Response | Promise<Response> | undefined;

/**
 * Answer the mail routes through `route`, and record every request. A route
 * that answers undefined falls through to a 204 for the thread actions and
 * a pending promise for the rest.
 */
export function serveMail(route: MailRoute): SeenRequest[] {
  const seen: SeenRequest[] = [];
  mockFetch((url, init) => {
    const parsed = new URL(url, "http://localhost");
    const body = init?.body;
    const request: SeenRequest = {
      method: init?.method ?? "GET",
      path: decodeURIComponent(parsed.pathname) + parsed.search,
      body: typeof body === "string" ? (JSON.parse(body) as unknown) : body,
    };
    seen.push(request);
    const answer = route(request);
    if (answer !== undefined) return answer;
    if (/\/(read|archive|restore)$/.test(parsed.pathname))
      return new Response(null, { status: 204 });
    return new Promise<Response>(() => {});
  });
  return seen;
}

export { jsonResponse };

/** Put the shared store back as a fresh page has it. */
export function resetMail(): void {
  mail.close();
  mail.leave();
  mail.discardNewMessage();
  mail.replies = {};
  mail.pending = {};
  mail.problems = {};
  mail.list = null;
  mail.threads = [];
  mail.next = null;
  mail.loaded = false;
  mail.loadError = null;
  mail.loadingMore = false;
  mail.moreError = null;
  vi.restoreAllMocks();
}
