// ── User-facing error messages ───────────────────────────────────────
//
// Raw errors (`ApiError`'s "404 Not Found: no run …", a fetch `TypeError`)
// are for developers. Every place that tells the user something failed goes
// through `userErrorMessage`, which logs the raw error to the console and
// returns plain language: what failed, and what to do about it.

import { ApiError } from "./api";

export interface ErrorMessageOptions {
  /** What failed, as a full sentence: "Couldn't load sessions." */
  action: string;
  /** Explanation for a 404, when "not found" means something specific here. */
  notFound?: string;
  /** Explanation for a server-side failure, when it means something specific here. */
  serverFault?: string;
}

const UNREACHABLE = "Residuum isn't reachable. Check that it's running, then try again.";
const SERVER_FAULT =
  "Residuum ran into a problem on its end. Try again; if it keeps happening, Residuum's logs have the details.";
const UNEXPECTED = "Something unexpected went wrong. Try again, or reload the page.";

/** Longest server-supplied message shown verbatim; anything longer is likely a dump. */
const MAX_SERVER_MESSAGE = 240;

/**
 * The message a server put in an error body (plain text, or `{"error": …}`
 * JSON), if it's short, human-readable text rather than a page or a dump.
 */
function serverMessage(body: string): string | null {
  let text = body.trim();
  if (text.startsWith("{")) {
    try {
      const parsed: unknown = JSON.parse(text);
      const field =
        typeof parsed === "object" && parsed !== null && "error" in parsed
          ? (parsed as { error: unknown }).error
          : null;
      text = typeof field === "string" ? field.trim() : "";
    } catch {
      return null;
    }
  }
  if (!text || text.length > MAX_SERVER_MESSAGE || text.includes("<") || text.includes("\n")) {
    return null;
  }
  const sentence = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(sentence) ? sentence : `${sentence}.`;
}

/** Plain-language reason for a failure, without the "what failed" part. */
function errorReason(err: unknown, opts: ErrorMessageOptions): string {
  // `fetch` rejects with a TypeError when the request never got a response.
  if (err instanceof TypeError) return UNREACHABLE;
  if (!(err instanceof ApiError)) return UNEXPECTED;
  switch (err.status) {
    case 404:
      return opts.notFound ?? "It wasn't found. It may have been removed.";
    case 401:
    case 403:
      return "Residuum refused the request. Reload the page and try again.";
    default:
      break;
  }
  if (err.status >= 500) return opts.serverFault ?? SERVER_FAULT;
  // Remaining 4xx replies explain what was wrong with the request (a
  // validation failure, a conflict), which is what the user needs to fix it.
  return serverMessage(err.body) ?? UNEXPECTED;
}

/**
 * Turn a caught error into a message for the user, logging the raw error to
 * the console so the detail isn't lost.
 */
export function userErrorMessage(err: unknown, opts: ErrorMessageOptions): string {
  return `${opts.action} ${userErrorReason(err, opts)}`;
}

/**
 * Like `userErrorMessage`, but only the reason, for copy that already says
 * what failed. `action` still labels the console log.
 */
export function userErrorReason(err: unknown, opts: ErrorMessageOptions): string {
  // eslint-disable-next-line no-console -- the raw error's only home once the user sees plain language
  console.error(opts.action, err);
  return errorReason(err, opts);
}
