import { describe, expect, it } from "vitest";
import { ApiError, validationFromApiError } from "./api";

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
