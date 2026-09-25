import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { userErrorMessage, userErrorReason } from "./errors";

describe("userErrorReason", () => {
  it("reports Residuum as unreachable for a fetch TypeError", () => {
    expect(userErrorReason(new TypeError("Failed to fetch"), { action: "Couldn't load." })).toBe(
      "Residuum isn't reachable. Check that it's running, then try again.",
    );
  });

  it("falls back to a generic message for anything that isn't an ApiError", () => {
    expect(userErrorReason(new Error("boom"), { action: "Couldn't load." })).toBe(
      "Something unexpected went wrong. Try again, or reload the page.",
    );
    expect(userErrorReason("not even an error", { action: "Couldn't load." })).toBe(
      "Something unexpected went wrong. Try again, or reload the page.",
    );
  });

  it("uses the caller's notFound override on a 404, else a generic one", () => {
    const err = new ApiError(404, "Not Found", "no such run");
    expect(userErrorReason(err, { action: "Couldn't load." })).toBe(
      "It wasn't found. It may have been removed.",
    );
    expect(
      userErrorReason(err, { action: "Couldn't load.", notFound: "That session is gone." }),
    ).toBe("That session is gone.");
  });

  it("treats 401 and 403 as a refusal to reload and retry", () => {
    for (const status of [401, 403]) {
      const err = new ApiError(status, "Unauthorized", "");
      expect(userErrorReason(err, { action: "Couldn't load." })).toBe(
        "Residuum refused the request. Reload the page and try again.",
      );
    }
  });

  it("uses the caller's serverFault override on a 5xx, else the generic server-fault message", () => {
    const err = new ApiError(500, "Internal Server Error", "stack trace garbage");
    expect(userErrorReason(err, { action: "Couldn't load." })).toBe(
      "Residuum ran into a problem on its end. Try again; if it keeps happening, Residuum's logs have the details.",
    );
    expect(
      userErrorReason(err, {
        action: "Couldn't load.",
        serverFault: "Saving failed on the server.",
      }),
    ).toBe("Saving failed on the server.");
  });

  it("shows a plain-text 4xx body verbatim, sentence-cased and punctuated", () => {
    const err = new ApiError(400, "Bad Request", "invalid model name");
    expect(userErrorReason(err, { action: "Couldn't save." })).toBe("Invalid model name.");
  });

  it("does not double-punctuate a body that already ends with punctuation", () => {
    const err = new ApiError(400, "Bad Request", "invalid model name!");
    expect(userErrorReason(err, { action: "Couldn't save." })).toBe("Invalid model name!");
  });

  it("extracts the error field from a JSON body", () => {
    const err = new ApiError(400, "Bad Request", JSON.stringify({ error: "port already in use" }));
    expect(userErrorReason(err, { action: "Couldn't save." })).toBe("Port already in use.");
  });

  it("shows a long 4xx validation message verbatim instead of swapping in a generic one", () => {
    // Regression: this used to be silently replaced once it crossed the old
    // 240-char cap, so a real validation error (e.g. HEARTBEAT.yml) was
    // never actually shown to the user.
    const longMessage =
      `HEARTBEAT.yml is invalid: ${"pulse configuration error detail. ".repeat(10)}`.trim();
    expect(longMessage.length).toBeGreaterThan(240);
    const err = new ApiError(400, "Bad Request", longMessage);
    const shown = userErrorReason(err, { action: "Couldn't reload." });
    expect(shown.startsWith("HEARTBEAT.yml is invalid:")).toBe(true);
    expect(shown.length).toBeGreaterThan(240);
  });

  it("shows a 4xx body with newlines verbatim instead of swapping in a generic one", () => {
    const err = new ApiError(400, "Bad Request", "line one\nline two");
    expect(userErrorReason(err, { action: "Couldn't save." })).toBe("Line one\nline two.");
  });

  it("does not filter a message that merely mentions a tag-like substring", () => {
    const err = new ApiError(400, "Bad Request", "field <name> is required");
    expect(userErrorReason(err, { action: "Couldn't save." })).toBe("Field <name> is required.");
  });

  it("filters an HTML error page and falls back to the generic message", () => {
    const doctype = new ApiError(
      502,
      "Bad Gateway",
      "<!DOCTYPE html><html><body><h1>502 Bad Gateway</h1></body></html>",
    );
    expect(userErrorReason(doctype, { action: "Couldn't load." })).toBe(
      "Residuum ran into a problem on its end. Try again; if it keeps happening, Residuum's logs have the details.",
    );

    const bareHtml = new ApiError(400, "Bad Request", "<html><body>nginx error page</body></html>");
    expect(userErrorReason(bareHtml, { action: "Couldn't load." })).toBe(
      "Something unexpected went wrong. Try again, or reload the page.",
    );
  });

  it("falls back to the generic message for an empty or unparsable body", () => {
    const empty = new ApiError(400, "Bad Request", "");
    expect(userErrorReason(empty, { action: "Couldn't save." })).toBe(
      "Something unexpected went wrong. Try again, or reload the page.",
    );
    const badJson = new ApiError(400, "Bad Request", "{not json");
    expect(userErrorReason(badJson, { action: "Couldn't save." })).toBe(
      "Something unexpected went wrong. Try again, or reload the page.",
    );
  });
});

describe("userErrorMessage", () => {
  it("prefixes the action onto the reason", () => {
    const err = new ApiError(404, "Not Found", "");
    expect(userErrorMessage(err, { action: "Couldn't load sessions." })).toBe(
      "Couldn't load sessions. It wasn't found. It may have been removed.",
    );
  });
});
