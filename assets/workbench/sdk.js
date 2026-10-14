// Residuum workbench SDK, injected into every HTML page a workbench artifact serves.
//
// An artifact page runs on the artifacts origin, which forwards `/api` to
// Residuum (docs/systems-usage/workbench.md, "API forwarding"), so the SDK
// talks to Residuum directly on the page's own origin: HTTP with relative
// URLs, live events over the hub socket, and one agent's frames over that
// agent's socket when the page asks for them. Artifacts belong to the team,
// so nothing defaults to an agent: every agent-specific call names one.
//
// The artifacts listener prepends `__RESIDUUM_ARTIFACT__`, `__RESIDUUM_VERSION__`,
// and `__RESIDUUM_FEATURES__` const declarations to this script before serving it,
// so this closure reads them from its enclosing scope.
(() => {
  "use strict";
  if (window.residuum) return;

  const ARTIFACT = __RESIDUUM_ARTIFACT__;
  const ARTIFACT_HEADER = "X-Residuum-Artifact";
  const nativeFetch = window.fetch.bind(window);

  // ── Handlers ─────────────────────────────────────────────────────────

  function addHandler(handlers, type, handler) {
    if (!handlers.has(type)) handlers.set(type, new Set());
    handlers.get(type).add(handler);
    return () => {
      handlers.get(type)?.delete(handler);
    };
  }

  function callHandler(handler, frame, what) {
    try {
      handler(frame);
    } catch (err) {
      console.error(`residuum: ${what} handler failed`, err);
    }
  }

  // Each handler registered for the frame's type, then each registered for "*".
  function emit(handlers, frame) {
    for (const key of [frame.type, "*"]) {
      for (const handler of [...(handlers.get(key) || [])]) {
        callHandler(handler, frame, `a "${key}"`);
      }
    }
  }

  // A `connection` handler hears the state the socket is already in, then
  // every change, so registering after the socket opened misses nothing.
  function replayConnection(handlers, handler, state) {
    if (state === null) return;
    queueMicrotask(() => {
      if (handlers.get("connection")?.has(handler)) {
        callHandler(handler, { type: "connection", state }, 'a "connection"');
      }
    });
  }

  // ── Calling the API ──────────────────────────────────────────────────

  // Unscoped paths whose home is the hub or the team rather than one agent,
  // mapped onto their scope. Every other unscoped path belongs to an agent.
  const HUB_PREFIXES = [
    "/api/secrets",
    "/api/agent-keys",
    "/api/a2a/keys",
    "/api/cloud/",
    "/api/update/",
    "/api/tracing/",
    "/api/shutdown",
    "/api/system/timezone",
    "/api/mcp-catalog",
  ];
  const TEAM_PREFIXES = ["/api/workbench/"];
  const SCOPED_PREFIXES = ["/api/hub/", "/api/team/", "/api/agents/"];

  function startsWithSegment(path, prefix) {
    if (!path.startsWith(prefix)) return false;
    if (prefix.endsWith("/")) return true;
    const next = path.charAt(prefix.length);
    return next === "" || next === "/";
  }

  function agentPathGuidance(method, pathname) {
    if (method === "POST" && /^\/api\/sessions\/?$/.test(pathname)) {
      return "A session runs on one agent. Start it with residuum.sessions.start({ agent, prompt }), which names the agent.";
    }
    if (method === "POST" && /^\/api\/model\/complete\/?$/.test(pathname)) {
      return "A model call runs on one agent's models. Make it with residuum.ask(prompt, { agent }), which names the agent.";
    }
    return `${pathname} belongs to one agent, so the path has to name it: /api/agents/<name>${pathname.slice("/api".length)}. The agents are listed at /api/hub/agents.`;
  }

  // `{ url }` to send, or `{ refusal }` for a path that isn't sent: one outside
  // the API, or one that belongs to an agent and names none.
  function resolveApiPath(method, path) {
    let url;
    try {
      url = new URL(path, location.href);
    } catch {
      return { refusal: `"${path}" isn't a valid path.` };
    }
    const { pathname, search } = url;
    if (url.origin !== location.origin || !(pathname === "/api" || pathname.startsWith("/api/"))) {
      return {
        refusal:
          "residuum.fetch only reaches Residuum's API: use a path starting with /api/, like /api/hub/status.",
      };
    }
    if (SCOPED_PREFIXES.some((prefix) => pathname.startsWith(prefix))) {
      return { url: pathname + search };
    }
    const rest = pathname.slice("/api".length);
    const hubRepo = ["hub", "team"].includes(url.searchParams.get("repo"));
    if (
      HUB_PREFIXES.some((prefix) => startsWithSegment(pathname, prefix)) ||
      (startsWithSegment(pathname, "/api/checkpoints") && hubRepo)
    ) {
      return { url: `/api/hub${rest}${search}` };
    }
    if (TEAM_PREFIXES.some((prefix) => startsWithSegment(pathname, prefix))) {
      return { url: `/api/team${rest}${search}` };
    }
    return { refusal: agentPathGuidance(method, pathname) };
  }

  function refusedResponse(message) {
    console.warn(`residuum.fetch: ${message}`);
    return new Response(JSON.stringify({ error: message }), {
      status: 400,
      statusText: "Bad Request",
      headers: { "Content-Type": "application/json" },
    });
  }

  // Also true for an object from another realm, whose prototype is that
  // realm's `Object.prototype`.
  function isPlainObject(value) {
    const proto = Object.getPrototypeOf(value);
    return proto === null || Object.getPrototypeOf(proto) === null;
  }

  // A plain object or array is sent as JSON; a string, `ArrayBuffer`, typed
  // array or `Blob` is sent unchanged.
  function encodeBody(body, headers) {
    if (body === undefined || body === null) return undefined;
    if (
      typeof body === "string" ||
      body instanceof ArrayBuffer ||
      ArrayBuffer.isView(body) ||
      (typeof Blob !== "undefined" && body instanceof Blob)
    ) {
      return body;
    }
    if (Array.isArray(body) || isPlainObject(body)) {
      if (!headers.has("Content-Type")) headers.set("Content-Type", "application/json");
      return JSON.stringify(body);
    }
    throw new TypeError(
      "residuum.fetch body must be a string, a plain object or array, an ArrayBuffer, a typed array, or a Blob",
    );
  }

  // At most `limit` tasks at once; the rest wait in the order they arrived.
  function lane(limit) {
    let active = 0;
    const waiting = [];
    return async (task) => {
      if (active >= limit) {
        // A finishing task hands its slot straight over, so a caller arriving
        // in between can't take it and push the count past the limit.
        await new Promise((resolve) => waiting.push(resolve));
      } else {
        active += 1;
      }
      try {
        return await task();
      } finally {
        const next = waiting.shift();
        if (next) next();
        else active -= 1;
      }
    };
  }

  // Ordinary requests share one lane so a page's bulk load can't flood the
  // instance, and slow model calls get a lane of their own so they never hold
  // up the page's other requests.
  const requestLane = lane(8);
  const modelCallLane = lane(4);
  const MODEL_CALL_PATH = /^\/api\/agents\/[^/]+\/model\/complete$/;

  function fetchApi(path, init = {}) {
    if (typeof path !== "string") {
      return Promise.reject(
        new TypeError("residuum.fetch path must be a string like '/api/hub/status'"),
      );
    }
    const method = (init.method || "GET").toUpperCase();
    const resolved = resolveApiPath(method, path);
    if (resolved.refusal !== undefined) return Promise.resolve(refusedResponse(resolved.refusal));
    let headers;
    let body;
    try {
      headers = new Headers(init.headers);
      body = encodeBody(init.body, headers);
    } catch (err) {
      return Promise.reject(err);
    }
    headers.set(ARTIFACT_HEADER, ARTIFACT);
    const request = { ...init, method, headers, body, credentials: "same-origin" };
    const run = MODEL_CALL_PATH.test(resolved.url.split("?", 1)[0]) ? modelCallLane : requestLane;
    return run(() => nativeFetch(resolved.url, request)).catch((err) => {
      if (err && err.name === "AbortError") throw err;
      console.error("residuum.fetch failed", method, resolved.url, err);
      throw new Error("Couldn't reach Residuum. Check that it's running, then try again.", {
        cause: err,
      });
    });
  }

  async function errorFrom(resp, fallback) {
    let body = null;
    try {
      body = await resp.json();
    } catch {
      // Not JSON; fall back to the plain message.
    }
    const err = new Error(body && typeof body.error === "string" ? body.error : fallback);
    if (body && typeof body.code === "string") err.code = body.code;
    // A refusal for an agent that isn't running names its state.
    if (body && typeof body.state === "string") err.state = body.state;
    err.status = resp.status;
    return err;
  }

  // ── Artifact state ───────────────────────────────────────────────────

  // Relative to the team directory, which is what the team file API addresses.
  function statePath() {
    return `workbench/${ARTIFACT}.state.json`;
  }

  async function stateGet() {
    const res = await fetchApi(`/api/team/workspace/file?path=${encodeURIComponent(statePath())}`);
    if (res.status === 404) return null;
    if (!res.ok) throw await errorFrom(res, `residuum.state.get failed with status ${res.status}`);
    const text = await res.text();
    try {
      return JSON.parse(text);
    } catch (err) {
      throw new Error(`residuum.state.get: saved state is not valid JSON (${err.message})`);
    }
  }

  async function stateSet(value) {
    const res = await fetchApi("/api/team/workspace/file", {
      method: "PUT",
      body: { path: statePath(), content: JSON.stringify(value) },
    });
    if (!res.ok) throw await errorFrom(res, `residuum.state.set failed with status ${res.status}`);
  }

  // ── Model calls ──────────────────────────────────────────────────────

  // A model call runs on one agent's models, so the request names the agent:
  // `residuum.ask(prompt, { agent })` or `residuum.ask({ agent, prompt, ... })`.
  function ask(promptOrRequest, options) {
    let body;
    if (typeof promptOrRequest === "string") {
      body = { prompt: promptOrRequest };
    } else if (promptOrRequest && typeof promptOrRequest === "object") {
      body = { ...promptOrRequest };
    } else {
      return Promise.reject(
        new TypeError("residuum.ask needs a prompt string or a request object"),
      );
    }
    const agent = (options && options.agent) ?? body.agent;
    delete body.agent;
    if (typeof agent !== "string" || agent === "") {
      return Promise.reject(
        new TypeError(
          "residuum.ask needs an agent: pass { agent } in the request or as the second argument",
        ),
      );
    }
    const path = `/api/agents/${encodeURIComponent(agent)}/model/complete`;
    return fetchApi(path, { method: "POST", body }).then((resp) =>
      resp
        .json()
        .catch(() => null)
        .then((data) => {
          if (!resp.ok)
            throw new Error(data && data.error ? data.error : `model call failed (${resp.status})`);
          return data;
        }),
    );
  }

  // ── Live connections ─────────────────────────────────────────────────

  // A socket on the page's own origin that reconnects with backoff for as
  // long as the page is open. Nothing is queued while it is down: `opened`
  // sends what the connection needs, again after every reconnect.
  function liveSocket(path, label, { keepalive, opened, received, changed }) {
    const url = `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}${path}`;
    let socket = null;
    let state = null;
    let delay = 1000;
    let ping = null;

    function setState(next) {
      if (state === next) return;
      if (next === "disconnected") {
        console.warn(`residuum: ${label} isn't connected; reconnecting`);
      }
      state = next;
      changed(next);
    }

    function connect() {
      const ws = new WebSocket(url);
      socket = ws;
      ws.onopen = () => {
        delay = 1000;
        if (keepalive) ping = setInterval(() => send({ type: "ping" }), 30000);
        // What the connection needs goes out before anyone hears it is up.
        opened();
        setState("connected");
      };
      ws.onmessage = (event) => {
        let frame;
        try {
          frame = JSON.parse(String(event.data));
        } catch (err) {
          console.warn(`residuum: unreadable frame from ${label}`, err);
          return;
        }
        received(frame);
      };
      ws.onclose = () => {
        socket = null;
        clearInterval(ping);
        setState("disconnected");
        setTimeout(connect, delay);
        delay = Math.min(delay * 1.5, 15000);
      };
    }

    function send(message) {
      if (socket !== null && socket.readyState === WebSocket.OPEN)
        socket.send(JSON.stringify(message));
    }

    connect();
    return {
      send,
      get state() {
        return state;
      },
    };
  }

  // ── Watching files ───────────────────────────────────────────────────

  // Normalizes like Residuum: `/`-separated, no empty or `.` segments, "" for
  // the whole namespace. Throws for paths outside it.
  function normalizePrefix(call, prefix) {
    if (typeof prefix !== "string") {
      throw new TypeError(`${call} prefix must be a workspace path like "team/wiki"`);
    }
    const segments = prefix.split("/");
    if (prefix.includes("\\") || prefix.startsWith("/") || segments[0].includes(":")) {
      throw new TypeError(
        `${call} can't watch "${prefix}": use a path relative to the workspace, like "team/wiki"`,
      );
    }
    const kept = [];
    for (const segment of segments) {
      if (segment === "" || segment === ".") continue;
      if (segment === "..")
        throw new TypeError(`${call} can't watch "${prefix}": ".." leaves the workspace`);
      kept.push(segment);
    }
    return kept.join("/");
  }

  const isWithin = (path, ancestor) =>
    ancestor === "" ||
    path === ancestor ||
    (path.startsWith(ancestor) && path[ancestor.length] === "/");
  // A change concerns a prefix when it is the prefix, lies under it, or is a
  // folder containing it (renaming or removing that folder carries it along).
  const concerns = (path, prefix) => isWithin(path, prefix) || isWithin(prefix, path);

  const WORKSPACE_FRAMES = new Set([
    "workspace_changed",
    "workspace_resync",
    "workspace_watch_unavailable",
  ]);

  function prefixesOf(watchers) {
    return [...new Set([...watchers].map((watcher) => watcher.prefix))].sort();
  }

  function deliverToWatchers(watchers, frame) {
    for (const watcher of [...watchers]) {
      let delivered = frame;
      if (frame.type === "workspace_changed") {
        const changes = frame.changes.filter((change) => concerns(change.path, watcher.prefix));
        if (changes.length === 0) continue;
        delivered = { type: frame.type, changes };
      }
      callHandler(watcher.handler, delivered, "a watch");
    }
  }

  // `sync` sends the watchers' prefixes to the socket that serves them.
  function addWatcher(watchers, prefix, handler, sync) {
    const watcher = { prefix, handler };
    watchers.add(watcher);
    sync();
    return () => {
      if (watchers.delete(watcher)) sync();
    };
  }

  // ── Hub events ───────────────────────────────────────────────────────

  const HUB_EVENT_TYPES = new Set(["artifact_updated", "artifact_removed", "connection"]);
  const hubHandlers = new Map();
  const teamWatchers = new Set();

  function on(type, handler) {
    if (typeof handler !== "function")
      throw new TypeError("residuum.on handler must be a function");
    if (type !== "*" && !HUB_EVENT_TYPES.has(type)) {
      throw new TypeError(
        `residuum.on hears Residuum's own events: "artifact_updated", "artifact_removed", "connection", or "*" for all three. "${String(type)}" comes from one agent: use residuum.agent("<name>").on("${String(type)}", handler).`,
      );
    }
    const off = addHandler(hubHandlers, type, handler);
    if (type === "connection") replayConnection(hubHandlers, handler, hub.state);
    return off;
  }

  function watch(prefix, handler) {
    if (typeof handler !== "function")
      throw new TypeError("residuum.watch handler must be a function");
    const normalized = normalizePrefix("residuum.watch", prefix);
    if (normalized !== "team" && !normalized.startsWith("team/")) {
      const instead =
        normalized === ""
          ? 'To follow one agent\'s whole workspace, use residuum.agent("<name>").watch("", handler).'
          : `"${prefix}" is in an agent's workspace: use residuum.agent("<name>").watch("${normalized}", handler).`;
      throw new TypeError(
        `residuum.watch follows team files: "team" or a path under it, like "team/wiki". ${instead}`,
      );
    }
    return addWatcher(teamWatchers, normalized, handler, () => {
      hub.send({ type: "watch_team", prefixes: prefixesOf(teamWatchers) });
    });
  }

  // Live reload: the page reloads when its own artifact changes, unless it
  // registered an `artifact_updated` handler to deal with that itself.
  function artifactEvent(frame) {
    emit(hubHandlers, frame);
    if (
      frame.type === "artifact_updated" &&
      frame.name === ARTIFACT &&
      !hubHandlers.get("artifact_updated")?.size
    ) {
      location.reload();
    }
  }

  // ── Agents ───────────────────────────────────────────────────────────

  const agentHandles = new Map();

  function agent(name) {
    if (typeof name !== "string" || name === "") {
      throw new TypeError('residuum.agent needs an agent\'s name, like residuum.agent("scout")');
    }
    let handle = agentHandles.get(name);
    if (handle === undefined) {
      handle = agentHandle(name);
      agentHandles.set(name, handle);
    }
    return handle;
  }

  function agentHandle(name) {
    const label = `residuum.agent(${JSON.stringify(name)})`;
    const handlers = new Map();
    const watchers = new Set();
    let socket = null;
    let connectedBefore = false;

    const syncWatch = () => {
      socket?.send({ type: "watch_workspace", prefixes: prefixesOf(watchers) });
    };

    // Opened on first use and kept for as long as the page is open.
    function connection() {
      socket ??= liveSocket(
        `/api/agents/${encodeURIComponent(name)}/ws`,
        `${name}'s live connection`,
        {
          keepalive: true,
          opened() {
            socket.send({ type: "set_verbose", enabled: true });
            if (watchers.size > 0) syncWatch();
            if (connectedBefore)
              deliverToWatchers(watchers, { type: "workspace_resync", reason: "reconnected" });
            connectedBefore = true;
          },
          received(frame) {
            if (frame.type === "pong") return;
            if (WORKSPACE_FRAMES.has(frame.type)) deliverToWatchers(watchers, frame);
            else emit(handlers, frame);
          },
          changed: (state) => emit(handlers, { type: "connection", state }),
        },
      );
      return socket;
    }

    return Object.freeze({
      name,
      on(type, handler) {
        if (typeof type !== "string")
          throw new TypeError(`${label}.on type must be a frame type, or "*"`);
        if (typeof handler !== "function")
          throw new TypeError(`${label}.on handler must be a function`);
        const off = addHandler(handlers, type, handler);
        const live = connection();
        if (type === "connection") replayConnection(handlers, handler, live.state);
        return off;
      },
      watch(prefix, handler) {
        if (typeof handler !== "function")
          throw new TypeError(`${label}.watch handler must be a function`);
        const normalized = normalizePrefix(`${label}.watch`, prefix);
        connection();
        return addWatcher(watchers, normalized, handler, syncWatch);
      },
    });
  }

  // ── Sessions ─────────────────────────────────────────────────────────

  // The page follows its artifact's sessions, on every agent, through the hub
  // socket's session relay: subscribed on the first start, and again after
  // every reconnect.
  const SUBSCRIBE_TIMEOUT_MS = 10000;
  const relay = { requested: false, active: false, activeBefore: false, waiting: [] };

  function noLiveConnection() {
    const err = new Error(
      "The session wasn't started: Residuum's live connection isn't available, so this page couldn't follow it. Check that Residuum is running and reachable, then try again.",
    );
    err.code = "no_live_connection";
    return err;
  }

  function relayReady() {
    if (relay.active) return Promise.resolve();
    if (!relay.requested) {
      relay.requested = true;
      hub.send({ type: "subscribe_artifact_sessions", artifact: ARTIFACT });
    }
    return new Promise((resolve, reject) => {
      const ready = () => {
        clearTimeout(timer);
        resolve();
      };
      const timer = setTimeout(() => {
        relay.waiting = relay.waiting.filter((waiter) => waiter !== ready);
        reject(noLiveConnection());
      }, SUBSCRIBE_TIMEOUT_MS);
      relay.waiting.push(ready);
    });
  }

  function relaySubscribed() {
    if (!relay.requested || relay.active) return;
    relay.active = true;
    for (const ready of relay.waiting.splice(0)) ready();
    // Frames sent while the socket was down are lost.
    if (relay.activeBefore) void resyncSessions();
    relay.activeBefore = true;
  }

  // Two agents can hold sessions at the same address, so a session is
  // identified by (agent, address).
  const sessionKey = (agentName, address) => `${agentName}\n${address}`;
  const sessionRoutes = new Map();

  // A session's first frames can arrive before the start request's reply says
  // which address is the page's. While any start is in flight, frames no handle
  // claims are kept here and handed to the handle that turns out to own them.
  let startsInFlight = 0;
  let unclaimed = [];

  function sessionAddress(frame) {
    if (typeof frame.address === "string") return frame.address;
    return frame.session && typeof frame.session.address === "string"
      ? frame.session.address
      : null;
  }

  function routeSessionFrame(agentName, frame) {
    if (typeof agentName !== "string" || !frame || typeof frame.type !== "string") return;
    const address = sessionAddress(frame);
    if (address === null) return;
    const key = sessionKey(agentName, address);
    const route = sessionRoutes.get(key);
    if (route) route.emit(frame);
    else if (startsInFlight > 0) unclaimed.push({ key, frame });
  }

  // Reads one agent's sessions for this artifact and hands each of `routes`
  // `resync` with its session as listed now, or the reason the list couldn't
  // be read. Shared by the bulk resync below and a single freshly followed
  // session, which wants its current state without waiting for a lag or a
  // reconnect.
  async function resyncRoutes(agentName, routes) {
    let listing = null;
    let error = null;
    try {
      const resp = await fetchApi(
        `/api/agents/${encodeURIComponent(agentName)}/sessions?artifact=${encodeURIComponent(ARTIFACT)}`,
      );
      if (!resp.ok) throw await errorFrom(resp, `Couldn't read ${agentName}'s sessions.`);
      listing = await resp.json();
    } catch (err) {
      console.error(`residuum: couldn't read ${agentName}'s sessions after missing frames`, err);
      error = err.message;
    }
    for (const route of routes) {
      if (listing === null) {
        route.emit({ type: "resync", session: null, error });
        continue;
      }
      const mine = (session) => session.address === route.address;
      const session = listing.live.find(mine) ?? listing.completed.find(mine) ?? null;
      route.emit({ type: "resync", session });
    }
  }

  // Lost frames can't be replayed, so each handle hears `resync` with its
  // session as Residuum lists it now (`null` when it isn't listed), or the
  // reason the list couldn't be read.
  async function resyncSessions() {
    const byAgent = new Map();
    for (const route of sessionRoutes.values()) {
      if (!byAgent.has(route.agent)) byAgent.set(route.agent, []);
      byAgent.get(route.agent).push(route);
    }
    await Promise.all([...byAgent].map(([agentName, routes]) => resyncRoutes(agentName, routes)));
  }

  // `early` holds the frames that arrived before the handle existed. Each
  // handler registered for their type gets them first, so `session_started`
  // (which carries the run id) and early output aren't lost.
  function sessionHandle(agentName, address, early) {
    const handlers = new Map();
    sessionRoutes.set(sessionKey(agentName, address), {
      agent: agentName,
      address,
      emit: (frame) => emit(handlers, frame),
    });
    const path = `/api/agents/${encodeURIComponent(agentName)}/sessions/${encodeURIComponent(address)}`;
    return Object.freeze({
      agent: agentName,
      address,
      on(type, handler) {
        if (typeof handler !== "function")
          throw new TypeError("session.on handler must be a function");
        const off = addHandler(handlers, type, handler);
        const missed = early.filter((frame) => type === "*" || frame.type === type);
        if (missed.length > 0) {
          queueMicrotask(() => {
            for (const frame of missed) {
              if (!handlers.get(type)?.has(handler)) return;
              callHandler(handler, frame, "a session");
            }
          });
        }
        return off;
      },
      async send(text) {
        if (typeof text !== "string" || text.trim() === "") {
          throw new TypeError("session.send needs a non-empty string");
        }
        const resp = await fetchApi(`${path}/messages`, {
          method: "POST",
          body: { content: text },
        });
        if (!resp.ok) throw await errorFrom(resp, `Couldn't message session ${address}.`);
        return (await resp.json()).outcome;
      },
      async stop() {
        const resp = await fetchApi(`${path}/stop`, { method: "POST" });
        if (!resp.ok) throw await errorFrom(resp, `Couldn't stop session ${address}.`);
      },
    });
  }

  // A session runs on one agent, so the start names it. The page subscribes
  // first, so it hears the session from its first frame on whichever agent
  // runs it; without a live connection it doesn't start a session it couldn't
  // follow.
  async function startSession(options) {
    if (!options || typeof options.agent !== "string" || options.agent === "") {
      throw new TypeError(
        "residuum.sessions.start needs { agent, prompt }: name the agent that runs the session",
      );
    }
    if (typeof options.prompt !== "string" || options.prompt.trim() === "") {
      throw new TypeError("residuum.sessions.start needs { prompt } with a non-empty prompt");
    }
    const agentName = options.agent;
    const body = { prompt: options.prompt };
    for (const key of ["context", "skill", "model"]) {
      if (options[key] !== undefined) body[key] = options[key];
    }
    await relayReady();
    startsInFlight += 1;
    let address = null;
    let early = [];
    try {
      const resp = await fetchApi(`/api/agents/${encodeURIComponent(agentName)}/sessions`, {
        method: "POST",
        body,
      });
      if (!resp.ok) throw await errorFrom(resp, "Couldn't start the session.");
      address = (await resp.json()).address;
    } finally {
      // Still counted as in flight until here, so no frame for this session
      // slips past both the buffer and its handle.
      startsInFlight -= 1;
      if (address !== null) {
        const key = sessionKey(agentName, address);
        early = unclaimed.filter((entry) => entry.key === key).map((entry) => entry.frame);
      }
      if (startsInFlight === 0) unclaimed = [];
    }
    return sessionHandle(agentName, address, early);
  }

  // A session already running when the page opens, or still running across a
  // reload, has no handle: `residuum.sessions.start` only ever hears a
  // session from its own start request on. `follow` subscribes to it
  // directly, by (agent, address), over the same relay (`subscribe_session`,
  // not the artifact-wide `subscribe_artifact_sessions` a start registers),
  // and resyncs it at once so the handle has the session's current state
  // without waiting for a lag or a reconnect to ask for it.
  const followedSessions = new Map();

  function sendFollowSubscriptions() {
    for (const { agent: agentName, address } of followedSessions.values()) {
      hub.send({ type: "subscribe_session", agent: agentName, address });
    }
  }

  // The subscription ends with the connection, so a reconnect's frames are
  // lost the same way a lag's are: resync once it is active again, but not
  // the first time, when the handle's own resync below already covers it.
  function sessionSubscribed(agentName, address) {
    const key = sessionKey(agentName, address);
    const followed = followedSessions.get(key);
    if (!followed) return;
    if (followed.activeBefore) {
      const route = sessionRoutes.get(key);
      if (route) void resyncRoutes(agentName, [route]);
    }
    followed.activeBefore = true;
  }

  function followSession(agentName, address) {
    if (typeof agentName !== "string" || agentName === "") {
      throw new TypeError(
        "residuum.sessions.follow needs an agent: residuum.sessions.follow(agent, address)",
      );
    }
    if (typeof address !== "string" || address === "") {
      throw new TypeError(
        "residuum.sessions.follow needs a session address: residuum.sessions.follow(agent, address)",
      );
    }
    const key = sessionKey(agentName, address);
    if (!followedSessions.has(key)) {
      followedSessions.set(key, { agent: agentName, address, activeBefore: false });
      hub.send({ type: "subscribe_session", agent: agentName, address });
    }
    const handle = sessionHandle(agentName, address, []);
    const route = sessionRoutes.get(key);
    if (route) void resyncRoutes(agentName, [route]);
    return handle;
  }

  // ── The hub socket ───────────────────────────────────────────────────

  let hubConnectedBefore = false;

  function hubOpened() {
    if (teamWatchers.size > 0) hub.send({ type: "watch_team", prefixes: prefixesOf(teamWatchers) });
    if (relay.requested) hub.send({ type: "subscribe_artifact_sessions", artifact: ARTIFACT });
    sendFollowSubscriptions();
    if (hubConnectedBefore)
      deliverToWatchers(teamWatchers, { type: "workspace_resync", reason: "reconnected" });
    hubConnectedBefore = true;
  }

  function hubFrame(frame) {
    switch (frame.type) {
      case "artifact_updated":
      case "artifact_removed":
        artifactEvent(frame);
        return;
      case "workspace_changed":
      case "workspace_resync":
      case "workspace_watch_unavailable":
        deliverToWatchers(teamWatchers, frame);
        return;
      case "subscribed":
        if (frame.kind === "artifact_sessions" && frame.artifact === ARTIFACT) relaySubscribed();
        else if (frame.kind === "session") sessionSubscribed(frame.agent, frame.address);
        return;
      case "session_frame":
        routeSessionFrame(frame.agent, frame.frame);
        return;
      case "session_relay_lagged":
        void resyncSessions();
        return;
      default:
        // The rest of the hub's frames are for the Residuum app.
        return;
    }
  }

  // Opened as the page loads, for live reload, artifact events, team watches
  // and sessions.
  const hub = liveSocket("/api/hub/ws", "Residuum's live connection", {
    keepalive: false,
    opened: hubOpened,
    received: hubFrame,
    changed(state) {
      if (state !== "connected") relay.active = false;
      emit(hubHandlers, { type: "connection", state });
    },
  });

  window.residuum = Object.freeze({
    artifact: ARTIFACT,
    version: __RESIDUUM_VERSION__,
    features: Object.freeze(__RESIDUUM_FEATURES__.slice()),
    fetch: fetchApi,
    ask,
    on,
    watch,
    agent,
    state: Object.freeze({ get: stateGet, set: stateSet }),
    sessions: Object.freeze({ start: startSession, follow: followSession }),
  });
})();
