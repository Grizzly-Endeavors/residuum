import { describe, expect, it } from "vitest";
import { ApiError, validationFromApiError, workspaceConflictFromApiError } from "./api";

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
