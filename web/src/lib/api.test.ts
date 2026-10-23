import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, uploadFile, validationFromApiError, workspaceConflictFromApiError } from "./api";

describe("validationFromApiError", () => {
  it("returns the validation message from a 400 body", () => {
    const err = new ApiError(
      400,
      "Bad Request",
      JSON.stringify({ valid: false, error: "invalid TOML at line 3" }),
    );
    expect(validationFromApiError(err)).toEqual({
      valid: false,
      error: "invalid TOML at line 3",
    });
  });

  it("ignores errors that are not a 400", () => {
    const err = new ApiError(500, "Internal Server Error", JSON.stringify({ error: "boom" }));
    expect(validationFromApiError(err)).toBeNull();
  });

  it("ignores a 400 whose body carries no error message", () => {
    expect(validationFromApiError(new ApiError(400, "Bad Request", "not json"))).toBeNull();
    expect(validationFromApiError(new ApiError(400, "Bad Request", "{}"))).toBeNull();
  });

  it("ignores anything that is not an ApiError", () => {
    expect(validationFromApiError(new Error("network down"))).toBeNull();
  });
});

describe("workspaceConflictFromApiError", () => {
  it("returns the conflict details from a 412 body", () => {
    const err = new ApiError(
      412,
      "Precondition Failed",
      JSON.stringify({
        error: "file has changed since it was last read",
        current_version: "abc123",
      }),
    );
    expect(workspaceConflictFromApiError(err)).toEqual({
      error: "file has changed since it was last read",
      currentVersion: "abc123",
    });
  });

  it("accepts a null current_version (the file was deleted)", () => {
    const err = new ApiError(
      412,
      "Precondition Failed",
      JSON.stringify({ error: "file already exists", current_version: null }),
    );
    expect(workspaceConflictFromApiError(err)).toEqual({
      error: "file already exists",
      currentVersion: null,
    });
  });

  it("ignores errors that are not a 412", () => {
    const err = new ApiError(
      500,
      "Internal Server Error",
      JSON.stringify({ error: "boom", current_version: "x" }),
    );
    expect(workspaceConflictFromApiError(err)).toBeNull();
  });

  it("ignores a 412 whose body doesn't carry the expected shape", () => {
    expect(
      workspaceConflictFromApiError(new ApiError(412, "Precondition Failed", "not json")),
    ).toBeNull();
    expect(
      workspaceConflictFromApiError(new ApiError(412, "Precondition Failed", "{}")),
    ).toBeNull();
  });

  it("ignores anything that is not an ApiError", () => {
    expect(workspaceConflictFromApiError(new Error("network down"))).toBeNull();
  });
});

describe("uploadFile", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const staged = { id: "upload_1", filename: "notes.txt", size: 5 };

  it("sends the file's bytes under its name, with no signal unless one is given", async () => {
    const fetchMock = vi.fn((_target: string, _init: RequestInit) =>
      Promise.resolve(Response.json(staged)),
    );
    vi.stubGlobal("fetch", fetchMock);

    const file = new Blob(["notes"], { type: "text/plain" });
    expect(await uploadFile(file, "notes.txt")).toEqual(staged);

    const [target, init] = fetchMock.mock.calls[0] ?? [];
    expect(target).toBe("/api/hub/uploads?filename=notes.txt");
    expect(init).toMatchObject({
      method: "POST",
      headers: { "Content-Type": "text/plain" },
      body: file,
    });
    expect(init).not.toHaveProperty("signal");
  });

  it("hands the signal to the request, so aborting it cancels the upload", async () => {
    const fetchMock = vi.fn(
      (_target: string, init: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init.signal?.addEventListener("abort", () => {
            reject(new DOMException("The operation was aborted.", "AbortError"));
          });
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const controller = new AbortController();
    const upload = uploadFile(new Blob(["notes"]), "notes.txt", { signal: controller.signal });
    expect(fetchMock.mock.calls[0]?.[1].signal).toBe(controller.signal);

    controller.abort();
    await expect(upload).rejects.toMatchObject({ name: "AbortError" });
  });
});
