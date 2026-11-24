import { ApiError, UPLOAD_LIMIT_LABEL } from "./api";

export interface ErrorMessageOptions {
  action: string;
  notFound?: string;
  serverFault?: string;
}

const UNREACHABLE = "Residuum isn't reachable. Check that it's running, then try again.";
const SERVER_FAULT =
  "Residuum ran into a problem on its end. Try again; if it keeps happening, Residuum's logs have the details.";
const UNEXPECTED = "Something unexpected went wrong. Try again, or reload the page.";

function looksLikeHtmlPage(text: string): boolean {
  const head = text.slice(0, 200).toLowerCase();
  return head.includes("<!doctype") || head.includes("<html");
}

function serverMessage(body: string): string | null {
  let text = body.trim();
  if (text.startsWith("{")) {
    try {
      const parsed: unknown = JSON.parse(text);
      let field: unknown = null;
      if (typeof parsed === "object" && parsed !== null) {
        if ("message" in parsed && typeof (parsed as { message: unknown }).message === "string") {
          field = (parsed as { message: string }).message;
        } else if ("error" in parsed && typeof (parsed as { error: unknown }).error === "string") {
          field = (parsed as { error: string }).error;
        }
      }
      text = typeof field === "string" ? field.trim() : "";
    } catch {
      return null;
    }
  }
  if (!text || looksLikeHtmlPage(text)) {
    return null;
  }
  const sentence = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(sentence) ? sentence : `${sentence}.`;
}

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
    case 413:
      // A proxy in front of Residuum can answer 413 with an HTML page, so the limit is stated here rather than read from the body.
      return `That file is larger than the ${UPLOAD_LIMIT_LABEL} upload limit.`;
    default:
      break;
  }
  if (err.status >= 500) return opts.serverFault ?? SERVER_FAULT;
  return serverMessage(err.body) ?? UNEXPECTED;
}

export function userErrorMessage(err: unknown, opts: ErrorMessageOptions): string {
  return `${opts.action} ${userErrorReason(err, opts)}`;
}

export function userErrorReason(err: unknown, opts: ErrorMessageOptions): string {
  // eslint-disable-next-line no-console -- the raw error's only home once the user sees plain language
  console.error(opts.action, err);
  return errorReason(err, opts);
}
