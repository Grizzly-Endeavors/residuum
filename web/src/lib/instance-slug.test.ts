import { describe, expect, it } from "vitest";
import { instanceName, isInstanceSlug, withValidSlugs } from "./instance-slug";

describe("isInstanceSlug", () => {
  it.each(["a", "laptop", "my-desktop", "a1", "0", "a-b-c", "x".repeat(24)])(
    "accepts %s",
    (slug) => {
      expect(isInstanceSlug(slug)).toBe(true);
    },
  );

  it.each([
    "",
    "-a",
    "a-",
    "-",
    "A",
    "Laptop",
    "a_b",
    "a b",
    "a/../b",
    "a/b",
    "..",
    'x"onclick=1',
    "<img>",
    "a\n",
    "é",
    "x".repeat(25),
  ])("rejects %j", (slug) => {
    expect(isInstanceSlug(slug)).toBe(false);
  });

  it("rejects values that are not strings", () => {
    expect(isInstanceSlug(null)).toBe(false);
    expect(isInstanceSlug(undefined)).toBe(false);
    expect(isInstanceSlug(7)).toBe(false);
  });
});

describe("withValidSlugs", () => {
  it("drops entries with a bad slug and keeps the order of the rest", () => {
    const kept = withValidSlugs([{ slug: "a" }, { slug: "a/../b" }, { slug: "b" }]);
    expect(kept.map((entry) => entry.slug)).toEqual(["a", "b"]);
  });
});

describe("instanceName", () => {
  it("prefers the display name and falls back to the slug", () => {
    expect(instanceName({ slug: "laptop", display_name: " Bear's laptop " })).toBe("Bear's laptop");
    expect(instanceName({ slug: "laptop", display_name: "  " })).toBe("laptop");
  });
});
