import { readFileSync } from "node:fs";
import type { IncomingMessage, OutgoingHttpHeaders, ServerResponse } from "node:http";
import { resolve } from "node:path";
import { WEB_ROOT } from "./assets";

export const APP_SHELL_CSP = readFileSync(resolve(WEB_ROOT, "app-shell-csp.txt"), "utf-8").trim();

function isHtml(contentType: unknown): boolean {
  const value: unknown = Array.isArray(contentType) ? (contentType as unknown[])[0] : contentType;
  return typeof value === "string" && value.split(";")[0]?.trim().toLowerCase() === "text/html";
}

function contentTypeIn(headers: OutgoingHttpHeaders): unknown {
  return Object.entries(headers).find(([name]) => name.toLowerCase() === "content-type")?.[1];
}

export function appShellPolicy(_req: IncomingMessage, res: ServerResponse, next: () => void): void {
  const writeHead = res.writeHead.bind(res) as (...args: unknown[]) => ServerResponse;
  res.writeHead = ((...args: unknown[]) => {
    const given = args.find(
      (arg): arg is OutgoingHttpHeaders =>
        typeof arg === "object" && arg !== null && !Array.isArray(arg),
    );
    const contentType =
      (given === undefined ? undefined : contentTypeIn(given)) ?? res.getHeader("content-type");
    if (isHtml(contentType)) {
      res.setHeader("Content-Security-Policy", APP_SHELL_CSP);
    }
    return writeHead(...args);
  }) as typeof res.writeHead;
  next();
}
