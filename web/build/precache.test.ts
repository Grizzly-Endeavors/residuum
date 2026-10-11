import { describe, expect, it } from "vitest";
import { isPrecached, planPrecache, type BuildFile } from "./precache";

const text = (path: string, content: string): BuildFile => ({
  path,
  bytes: new TextEncoder().encode(content),
});

const BUILD: BuildFile[] = [
  text("index.html", "<html>shell</html>"),
  text("favicon.svg", "<svg/>"),
  text("assets/index-abc.js", "main"),
  text("assets/index-abc.css", "styles"),
  text("assets/SettingsModal-def.js", "lazy chunk"),
  text("assets/onest-latin-400-normal-ghi.woff2", "font"),
  text("icons/icon-192.png", "png"),
  text("manifest.webmanifest", "{}"),
  text("mcp-catalog.json", "[]"),
  text("licenses/onest-OFL.txt", "license"),
];

describe("which files are precached", () => {
  it("takes the document, everything under assets and the icons", () => {
    expect(planPrecache(BUILD).urls).toEqual([
      "/assets/SettingsModal-def.js",
      "/assets/index-abc.css",
      "/assets/index-abc.js",
      "/assets/onest-latin-400-normal-ghi.woff2",
      "/favicon.svg",
      "/icons/icon-192.png",
      "/index.html",
    ]);
  });

  it("leaves out what the hub serves on demand and the worker itself", () => {
    for (const path of [
      "manifest.webmanifest",
      "mcp-catalog.json",
      "licenses/onest-OFL.txt",
      "sw.js",
    ]) {
      expect(isPrecached(path), path).toBe(false);
    }
  });

  it("is not fooled by a file that only resembles a directory", () => {
    expect(isPrecached("assets-backup/old.js")).toBe(false);
    expect(isPrecached("icons.png")).toBe(false);
  });
});

describe("the worker's version", () => {
  it("is the same for the same build, whatever order the files are read in", () => {
    expect(planPrecache([...BUILD].reverse()).version).toBe(planPrecache(BUILD).version);
  });

  it("looks like a short hash", () => {
    expect(planPrecache(BUILD).version).toMatch(/^[0-9a-f]{12}$/);
  });

  it("changes when a hashed asset is added or renamed", () => {
    const base = planPrecache(BUILD).version;
    expect(planPrecache([...BUILD, text("assets/Extra-jkl.js", "x")]).version).not.toBe(base);
    const renamed = BUILD.map((file) =>
      file.path === "assets/index-abc.js" ? text("assets/index-xyz.js", "main") : file,
    );
    expect(planPrecache(renamed).version).not.toBe(base);
  });

  it("changes when a file that keeps its name changes", () => {
    const base = planPrecache(BUILD).version;
    for (const path of ["index.html", "icons/icon-192.png", "favicon.svg"]) {
      const edited = BUILD.map((file) => (file.path === path ? text(path, "different") : file));
      expect(planPrecache(edited).version, path).not.toBe(base);
    }
  });

  it("ignores files the worker doesn't precache", () => {
    const base = planPrecache(BUILD).version;
    const edited = BUILD.map((file) =>
      file.path === "mcp-catalog.json" ? text("mcp-catalog.json", "[1]") : file,
    );
    expect(planPrecache(edited).version).toBe(base);
  });
});
