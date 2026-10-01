import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SKIP_WAITING_MESSAGE } from "./protocol";
import { shellCacheName } from "./rules";

// The worker runs where `self`, `caches` and `fetch` are the worker's. These
// tests give it small stand-ins for them, import it fresh, and drive its
// listeners the way a browser does.

const ORIGIN = "https://residuum.test";
const VERSION = "aaaaaaaaaaaa";
const FILES = [
  "/index.html",
  "/assets/index-abc.js",
  "/assets/Settings-def.js",
  "/icons/icon-192.png",
];

/** A cache that remembers responses by path. */
class FakeCache {
  readonly entries = new Map<string, Response>();

  put(path: string, response: Response): Promise<void> {
    this.entries.set(path, response);
    return Promise.resolve();
  }

  match(path: string): Promise<Response | undefined> {
    return Promise.resolve(this.entries.get(path)?.clone());
  }

  delete(path: string): Promise<boolean> {
    return Promise.resolve(this.entries.delete(path));
  }
}

class FakeCacheStorage {
  readonly stored = new Map<string, FakeCache>();

  open(name: string): Promise<FakeCache> {
    let cache = this.stored.get(name);
    if (cache === undefined) {
      cache = new FakeCache();
      this.stored.set(name, cache);
    }
    return Promise.resolve(cache);
  }

  keys(): Promise<string[]> {
    return Promise.resolve([...this.stored.keys()]);
  }

  delete(name: string): Promise<boolean> {
    return Promise.resolve(this.stored.delete(name));
  }

  /** The first cache with the path, in the order the caches were made, as a browser looks. */
  async match(path: string): Promise<Response | undefined> {
    for (const cache of this.stored.values()) {
      const found = await cache.match(path);
      if (found !== undefined) return found;
    }
    return undefined;
  }
}

type Handler = (event: unknown) => void;
type Network = (request: Request | string, init?: RequestInit) => Promise<Response>;

const urlOf = (request: Request | string): string =>
  typeof request === "string" ? request : request.url;

interface Loaded {
  caches: FakeCacheStorage;
  claimed: () => boolean;
  skipped: () => boolean;
  /** Fire a fetch event for `request`; the answer the worker gave it, or null when it left the request alone. */
  fetchEvent: (request: Partial<Request> & { url: string }) => Promise<Response | null>;
  /** Fire `type` with `event`; settles when the worker's `waitUntil` work has. */
  fire: (type: "install" | "activate" | "message", event?: object) => Promise<void>;
}

async function loadWorker(options: {
  network: Network;
  files?: string[];
  version?: string;
  caches?: FakeCacheStorage;
}): Promise<Loaded> {
  const listeners = new Map<string, Handler>();
  const storage = options.caches ?? new FakeCacheStorage();
  let claimed = false;
  let skipped = false;
  vi.stubGlobal("self", {
    location: { origin: ORIGIN },
    addEventListener: (type: string, handler: Handler) => listeners.set(type, handler),
    clients: {
      claim: () => {
        claimed = true;
        return Promise.resolve();
      },
    },
    skipWaiting: () => {
      skipped = true;
      return Promise.resolve();
    },
  });
  vi.stubGlobal("caches", storage);
  vi.stubGlobal("fetch", options.network);
  vi.stubGlobal("__SW_PRECACHE__", options.files ?? FILES);
  vi.stubGlobal("__SW_VERSION__", options.version ?? VERSION);
  vi.resetModules();
  await import("./worker");

  const listener = (type: string): Handler => {
    const found = listeners.get(type);
    if (found === undefined) throw new Error(`the worker has no ${type} listener`);
    return found;
  };
  return {
    caches: storage,
    claimed: () => claimed,
    skipped: () => skipped,
    fetchEvent: (request) => {
      const given: { answer: Promise<Response> | null } = { answer: null };
      listener("fetch")({
        request: { method: "GET", mode: "cors", ...request },
        respondWith: (response: Promise<Response>) => {
          given.answer = response;
        },
      });
      return given.answer ?? Promise.resolve(null);
    },
    fire: async (type, event = {}) => {
      const work: Promise<unknown>[] = [];
      listener(type)({ ...event, waitUntil: (promise: Promise<unknown>) => work.push(promise) });
      await Promise.all(work);
    },
  };
}

const ok = (body = "body", init: ResponseInit = {}): Response => new Response(body, init);

/** A network that serves every file and records what was asked for. */
function serving(asked: string[] = []): Network {
  return (request) => {
    const url = urlOf(request);
    asked.push(url);
    return Promise.resolve(ok(`network ${url}`));
  };
}

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("installing", () => {
  it("holds every listed file in this version's cache", async () => {
    const worker = await loadWorker({ network: serving() });
    await worker.fire("install");
    const cache = worker.caches.stored.get(shellCacheName(VERSION));
    expect([...(cache?.entries.keys() ?? [])].sort()).toEqual([...FILES].sort());
  });

  it("asks for files that keep their name past the browser's HTTP cache, and hashed ones as usual", async () => {
    const seen: [string, RequestCache | undefined][] = [];
    const recording: Network = (request, init) => {
      seen.push([urlOf(request), init?.cache]);
      return Promise.resolve(ok());
    };
    const worker = await loadWorker({ network: recording });
    await worker.fire("install");
    expect(Object.fromEntries(seen)).toEqual({
      "/index.html": "reload",
      "/icons/icon-192.png": "reload",
      "/assets/index-abc.js": "default",
      "/assets/Settings-def.js": "default",
    });
  });

  it("fails when a file can't be had, so the active worker keeps serving", async () => {
    const network: Network = (request) =>
      Promise.resolve(
        urlOf(request) === "/assets/Settings-def.js" ? ok("", { status: 503 }) : ok(),
      );
    const worker = await loadWorker({ network });
    await expect(worker.fire("install")).rejects.toThrow(
      "couldn't precache /assets/Settings-def.js: the server answered 503",
    );
  });

  it("fails when a file turns out to be where a redirect led, such as a sign-in page", async () => {
    const redirected = Object.defineProperty(ok("<html>sign in</html>"), "redirected", {
      value: true,
    });
    const network: Network = (request) =>
      Promise.resolve(urlOf(request) === "/index.html" ? redirected : ok());
    const worker = await loadWorker({ network });
    await expect(worker.fire("install")).rejects.toThrow(
      "couldn't precache /index.html: the server redirected",
    );
  });

  it("never has more than six files on their way at once", async () => {
    const many = Array.from({ length: 20 }, (_, index) => `/assets/chunk-${String(index)}.js`);
    let inFlight = 0;
    let peak = 0;
    const network: Network = async () => {
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await Promise.resolve();
      await Promise.resolve();
      inFlight -= 1;
      return ok();
    };
    const worker = await loadWorker({ network, files: many });
    await worker.fire("install");
    expect(peak).toBe(6);
    expect(worker.caches.stored.get(shellCacheName(VERSION))?.entries.size).toBe(20);
  });
});

describe("page loads", () => {
  async function installed(network: Network): Promise<Loaded> {
    const worker = await loadWorker({ network: serving() });
    await worker.fire("install");
    vi.stubGlobal("fetch", network);
    return worker;
  }

  const navigation = { url: `${ORIGIN}/agent/atlas/files`, mode: "navigate" } as const;

  it("takes the network's answer", async () => {
    const worker = await installed(() => Promise.resolve(ok("live page")));
    const answer = await worker.fetchEvent(navigation);
    expect(await answer?.text()).toBe("live page");
  });

  it("falls back to the cached shell when the network fails", async () => {
    const worker = await installed(() => Promise.reject(new TypeError("Failed to fetch")));
    const answer = await worker.fetchEvent(navigation);
    expect(await answer?.text()).toBe("network /index.html");
  });

  it("falls back to the cached shell when the relay or a proxy says the hub is down", async () => {
    for (const status of [502, 503, 504]) {
      const worker = await installed(() => Promise.resolve(ok("agent offline", { status })));
      const answer = await worker.fetchEvent(navigation);
      expect(await answer?.text(), String(status)).toBe("network /index.html");
    }
  });

  it("leaves every other answer as it came, errors included", async () => {
    for (const status of [200, 401, 404, 500]) {
      const worker = await installed(() => Promise.resolve(ok("as sent", { status })));
      const answer = await worker.fetchEvent(navigation);
      expect(await answer?.text(), String(status)).toBe("as sent");
    }
  });

  it("never stores the live page", async () => {
    const worker = await installed(() => Promise.resolve(ok("live page with a switcher")));
    await worker.fetchEvent(navigation);
    const shell = await worker.caches.stored.get(shellCacheName(VERSION))?.match("/index.html");
    expect(await shell?.text()).toBe("network /index.html");
  });

  it("answers the hub's own error when there is no shell to show", async () => {
    const worker = await loadWorker({
      network: () => Promise.resolve(ok("agent offline", { status: 503 })),
    });
    const answer = await worker.fetchEvent(navigation);
    expect(answer?.status).toBe(503);

    const offline = await loadWorker({
      network: () => Promise.reject(new TypeError("Failed to fetch")),
    });
    expect((await offline.fetchEvent(navigation))?.type).toBe("error");
  });
});

describe("the app's files", () => {
  async function installed(network: Network, existing?: FakeCacheStorage): Promise<Loaded> {
    const worker = await loadWorker({ network: serving(), caches: existing });
    await worker.fire("install");
    vi.stubGlobal("fetch", network);
    return worker;
  }

  const never: Network = () => Promise.reject(new TypeError("offline"));

  it("come from the cache, without asking the network", async () => {
    const asked: string[] = [];
    const worker = await installed((request) => {
      asked.push(urlOf(request));
      return Promise.resolve(ok());
    });
    for (const path of ["/assets/index-abc.js", "/icons/icon-192.png"]) {
      const answer = await worker.fetchEvent({ url: `${ORIGIN}${path}` });
      expect(await answer?.text(), path).toBe(`network ${path}`);
    }
    expect(asked).toEqual([]);
  });

  it("come from the previous version's cache when this version doesn't hold them", async () => {
    const storage = new FakeCacheStorage();
    await (
      await storage.open(shellCacheName("old"))
    ).put("/assets/Settings-old.js", ok("old chunk"));
    const worker = await installed(never, storage);
    const answer = await worker.fetchEvent({ url: `${ORIGIN}/assets/Settings-old.js` });
    expect(await answer?.text()).toBe("old chunk");
  });

  it("prefer this version's copy of a file to an older one's", async () => {
    const storage = new FakeCacheStorage();
    await (await storage.open(shellCacheName("old"))).put("/icons/icon-192.png", ok("old icon"));
    const worker = await installed(never, storage);
    const answer = await worker.fetchEvent({ url: `${ORIGIN}/icons/icon-192.png` });
    expect(await answer?.text()).toBe("network /icons/icon-192.png");
  });

  it("go to the network when no cache has them", async () => {
    const worker = await installed(() => Promise.resolve(ok("from the network")));
    const answer = await worker.fetchEvent({ url: `${ORIGIN}/assets/Unlisted-zzz.js` });
    expect(await answer?.text()).toBe("from the network");
  });
});

describe("what it leaves alone", () => {
  it("does not answer the API, the sockets, webhooks, the cloud callback, other origins or writes", async () => {
    const worker = await loadWorker({ network: serving() });
    await worker.fire("install");
    const left: (Partial<Request> & { url: string })[] = [
      { url: `${ORIGIN}/api/hub/agents` },
      { url: `${ORIGIN}/api/agents/atlas/chat/history`, mode: "navigate" },
      { url: `${ORIGIN}/ws` },
      { url: `${ORIGIN}/webhook/github`, method: "POST" },
      { url: `${ORIGIN}/cloud/callback?token=abc`, mode: "navigate" },
      { url: "https://bear.workbench.example.com/assets/index-abc.js" },
      { url: `${ORIGIN}/assets/index-abc.js`, method: "POST" },
      { url: `${ORIGIN}/manifest.webmanifest` },
    ];
    for (const request of left) {
      expect(await worker.fetchEvent(request), request.url).toBeNull();
    }
  });

  it("puts nothing from the API in any cache however the page asks", async () => {
    const worker = await loadWorker({ network: serving() });
    await worker.fire("install");
    await worker.fetchEvent({ url: `${ORIGIN}/api/hub/agents` });
    for (const cache of worker.caches.stored.values()) {
      expect([...cache.entries.keys()].filter((path) => path.startsWith("/api"))).toEqual([]);
    }
  });
});

describe("activating", () => {
  it("keeps this version and the one it replaced, deletes older ones and other caches' neighbours, and takes over the page", async () => {
    const storage = new FakeCacheStorage();
    for (const name of [shellCacheName("v1"), shellCacheName("v2"), "someone-elses-cache"]) {
      await storage.open(name);
    }
    await (
      await storage.open("residuum-generations")
    ).put("/generations", Response.json({ current: "v2", previous: "v1" }));

    const worker = await loadWorker({ network: serving(), caches: storage, version: "v3" });
    await worker.fire("install");
    await worker.fire("activate");

    expect(await storage.keys()).toEqual([
      shellCacheName("v2"),
      "someone-elses-cache",
      "residuum-generations",
      shellCacheName("v3"),
    ]);
    const recorded = await (await storage.open("residuum-generations")).match("/generations");
    expect(await recorded?.json()).toEqual({ current: "v3", previous: "v2" });
    expect(worker.claimed()).toBe(true);
  });

  it("starts from nothing when what was recorded can't be read", async () => {
    const storage = new FakeCacheStorage();
    await storage.open(shellCacheName("v1"));
    await (await storage.open("residuum-generations")).put("/generations", ok("not json"));

    const worker = await loadWorker({ network: serving(), caches: storage });
    await worker.fire("install");
    await worker.fire("activate");

    const recorded = await (await storage.open("residuum-generations")).match("/generations");
    expect(await recorded?.json()).toEqual({ current: VERSION, previous: null });
    expect(await storage.keys()).not.toContain(shellCacheName("v1"));
  });
});

describe("taking over", () => {
  it("waits until the page says so", async () => {
    const worker = await loadWorker({ network: serving() });
    await worker.fire("install");
    await worker.fire("activate");
    expect(worker.skipped()).toBe(false);

    await worker.fire("message", { data: SKIP_WAITING_MESSAGE });
    expect(worker.skipped()).toBe(true);
  });

  it("ignores any other message", async () => {
    const worker = await loadWorker({ network: serving() });
    await worker.fire("message", { data: { type: "something-else" } });
    await worker.fire("message", { data: "skip-waiting" });
    expect(worker.skipped()).toBe(false);
  });
});
