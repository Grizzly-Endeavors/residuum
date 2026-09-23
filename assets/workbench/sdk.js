// Residuum workbench SDK, injected into every HTML page a workbench artifact serves.
//
// Artifacts run on their own origin, so they cannot call the gateway directly.
// Every call is relayed over postMessage to the Residuum web UI hosting the
// frame, which enforces what artifacts may reach.
//
// The artifacts listener prepends `__RESIDUUM_ARTIFACT__`, `__RESIDUUM_VERSION__`,
// and `__RESIDUUM_FEATURES__` const declarations to this script before serving it,
// so this closure reads them from its enclosing scope.
(() => {
  "use strict";
  if (window.residuum) return;

  const TAG = "residuum-workbench";
  const embedded = window.parent !== window;
  const pending = new Map();
  const handlers = new Map();
  let nextId = 0;
  let subscribed = false;

  const notEmbedded = () =>
    new Error(
      "This artifact is not open inside Residuum. Open it from the Workbench page to use the Residuum API.",
    );

  function post(message, transfer) {
    window.parent.postMessage({ tag: TAG, ...message }, "*", transfer);
  }

  function request(kind, payload, transfer) {
    if (!embedded) return Promise.reject(notEmbedded());
    const id = `req-${++nextId}`;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      post({ kind, id, ...payload }, transfer);
    });
  }

  // Returns `{ body, transfer }`. `body` is what's sent to the bridge: a
  // string, an ArrayBuffer, or a Blob — never JSON-encoded when it's already
  // binary. `transfer` lists the ArrayBuffers to hand off (rather than copy)
  // across postMessage's structured clone.
  function encodeBody(body, headers) {
    if (body === undefined || body === null) return { body: null, transfer: [] };
    if (typeof body === "string") return { body, transfer: [] };
    if (body instanceof ArrayBuffer) return { body, transfer: [body] };
    if (ArrayBuffer.isView(body)) {
      // A typed array or DataView may be a view into a larger, still-in-use
      // buffer, so its exact byte range is copied into a fresh buffer before
      // transferring — transferring the underlying buffer directly could
      // hand over bytes outside the view, or detach a buffer the caller
      // still holds other views into.
      const copy = body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength);
      return { body: copy, transfer: [copy] };
    }
    if (typeof Blob !== "undefined" && body instanceof Blob) return { body, transfer: [] };
    if (Object.getPrototypeOf(body) === Object.prototype || Array.isArray(body)) {
      if (!Object.keys(headers).some((k) => k.toLowerCase() === "content-type")) {
        headers["Content-Type"] = "application/json";
      }
      return { body: JSON.stringify(body), transfer: [] };
    }
    throw new TypeError(
      "residuum.fetch body must be a string, a plain object/array, an ArrayBuffer, a typed array, or a Blob",
    );
  }

  function fetchVia(path, init = {}) {
    if (typeof path !== "string") {
      return Promise.reject(new TypeError("residuum.fetch path must be a string like '/api/status'"));
    }
    const headers = { ...(init.headers || {}) };
    let encoded;
    try {
      encoded = encodeBody(init.body, headers);
    } catch (err) {
      return Promise.reject(err);
    }
    return request(
      "fetch",
      {
        path,
        method: (init.method || "GET").toUpperCase(),
        headers,
        body: encoded.body,
      },
      encoded.transfer,
    ).then(
      (r) => new Response(r.status === 204 || r.status === 304 ? null : r.body, {
        status: r.status,
        statusText: r.statusText,
        headers: r.headers,
      }),
    );
  }

  function statePath() {
    return `workbench/${__RESIDUUM_ARTIFACT__}.state.json`;
  }

  async function errorFromResponse(res, fallback) {
    let message;
    try {
      message = await res.text();
    } catch {
      message = "";
    }
    return new Error(message || fallback);
  }

  async function stateGet() {
    const res = await fetchVia(`/api/workspace/file?path=${encodeURIComponent(statePath())}`);
    if (res.status === 404) return null;
    if (!res.ok) {
      throw await errorFromResponse(res, `residuum.state.get failed with status ${res.status}`);
    }
    const text = await res.text();
    try {
      return JSON.parse(text);
    } catch (err) {
      throw new Error(`residuum.state.get: saved state is not valid JSON (${err.message})`);
    }
  }

  async function stateSet(value) {
    const res = await fetchVia("/api/workspace/file", {
      method: "PUT",
      body: { path: statePath(), content: JSON.stringify(value) },
    });
    if (!res.ok) {
      throw await errorFromResponse(res, `residuum.state.set failed with status ${res.status}`);
    }
  }

  function ask(promptOrRequest) {
    let body;
    if (typeof promptOrRequest === "string") {
      body = { prompt: promptOrRequest };
    } else if (promptOrRequest && typeof promptOrRequest === "object") {
      body = promptOrRequest;
    } else {
      return Promise.reject(
        new TypeError("residuum.ask needs a prompt string or a request object"),
      );
    }
    return fetchVia("/api/model/complete", { method: "POST", body }).then((resp) =>
      resp
        .json()
        .catch(() => null)
        .then((data) => {
          if (!resp.ok) {
            throw new Error(
              data && data.error ? data.error : `model call failed (${resp.status})`,
            );
          }
          return data;
        }),
    );
  }

  function subscribe() {
    if (embedded && !subscribed) {
      subscribed = true;
      post({ kind: "subscribe" });
    }
  }

  function on(type, handler) {
    if (typeof handler !== "function") throw new TypeError("residuum.on handler must be a function");
    if (!handlers.has(type)) handlers.set(type, new Set());
    handlers.get(type).add(handler);
    subscribe();
    return () => handlers.get(type)?.delete(handler);
  }

  // ── Sessions ──────────────────────────────────────────────────────────

  // Replies to another client's session commands, not activity in the session.
  const COMMAND_REPLY_FRAMES = new Set([
    "session_message_delivered",
    "session_stop_requested",
    "session_command_failed",
  ]);

  // Every session frame names its session's address; `session_started`
  // carries it inside the session summary.
  function sessionFrameAddress(frame) {
    if (typeof frame.type !== "string" || !frame.type.startsWith("session_")) return null;
    if (COMMAND_REPLY_FRAMES.has(frame.type)) return null;
    if (typeof frame.address === "string") return frame.address;
    return frame.session && typeof frame.session.address === "string" ? frame.session.address : null;
  }

  // A session's first frames can arrive before the start request's reply
  // says which address is ours. While any start is in flight, session frames
  // are kept here and handed to the handle that turns out to own them.
  let startsInFlight = 0;
  let earlyFrames = [];
  const sessionRouters = new Map();

  function routeSessionFrame(frame) {
    const address = sessionFrameAddress(frame);
    if (address === null) return;
    const route = sessionRouters.get(address);
    if (route) route(frame);
    else if (startsInFlight > 0) earlyFrames.push(frame);
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
    err.status = resp.status;
    return err;
  }

  // `buffered` holds the frames that arrived before the handle existed. Each
  // handler registered for their type gets them first, so `session_started`
  // (which carries the run id) and early output aren't lost.
  function sessionHandle(address, buffered) {
    const own = new Map();
    sessionRouters.set(address, (frame) => {
      for (const key of [frame.type, "*"]) {
        for (const handler of own.get(key) || []) {
          try {
            handler(frame);
          } catch (err) {
            console.error("session handler failed", err);
          }
        }
      }
    });
    const path = `/api/sessions/${encodeURIComponent(address)}`;
    return Object.freeze({
      address,
      on(type, handler) {
        if (typeof handler !== "function") {
          throw new TypeError("session.on handler must be a function");
        }
        if (!own.has(type)) own.set(type, new Set());
        own.get(type).add(handler);
        const missed = buffered.filter((frame) => type === "*" || frame.type === type);
        if (missed.length > 0) {
          queueMicrotask(() => {
            for (const frame of missed) {
              if (!own.get(type)?.has(handler)) return;
              try {
                handler(frame);
              } catch (err) {
                console.error("session handler failed", err);
              }
            }
          });
        }
        return () => own.get(type)?.delete(handler);
      },
      async send(text) {
        if (typeof text !== "string" || text.trim() === "") {
          throw new TypeError("session.send needs a non-empty string");
        }
        const resp = await fetchVia(`${path}/messages`, { method: "POST", body: { content: text } });
        if (!resp.ok) throw await errorFrom(resp, `Couldn't message session ${address}.`);
        return (await resp.json()).outcome;
      },
      async stop() {
        const resp = await fetchVia(`${path}/stop`, { method: "POST" });
        if (!resp.ok) throw await errorFrom(resp, `Couldn't stop session ${address}.`);
      },
    });
  }

  async function startSession(options) {
    if (!options || typeof options.prompt !== "string" || options.prompt.trim() === "") {
      throw new TypeError("residuum.sessions.start needs { prompt } with a non-empty prompt");
    }
    const body = { prompt: options.prompt };
    for (const key of ["context", "skill", "model"]) {
      if (options[key] !== undefined) body[key] = options[key];
    }
    subscribe();
    startsInFlight += 1;
    let address = null;
    let buffered = [];
    try {
      const resp = await fetchVia("/api/sessions", { method: "POST", body });
      if (!resp.ok) throw await errorFrom(resp, "Couldn't start the session.");
      address = (await resp.json()).address;
    } finally {
      // Still counted as in flight until here, so no frame for this session
      // slips past both the buffer and its handle.
      startsInFlight -= 1;
      if (address !== null) {
        buffered = earlyFrames.filter((frame) => sessionFrameAddress(frame) === address);
      }
      if (startsInFlight === 0) earlyFrames = [];
    }
    return sessionHandle(address, buffered);
  }

  function dispatch(frame) {
    routeSessionFrame(frame);
    for (const key of [frame.type, "*"]) {
      for (const handler of handlers.get(key) || []) {
        try {
          handler(frame);
        } catch (err) {
          console.error("residuum.on handler failed", err);
        }
      }
    }
  }

  window.addEventListener("message", (event) => {
    if (event.source !== window.parent) return;
    const msg = event.data;
    if (!msg || msg.tag !== TAG) return;
    if (msg.kind === "event") {
      dispatch(msg.frame);
      return;
    }
    const entry = pending.get(msg.id);
    if (!entry) return;
    pending.delete(msg.id);
    if (msg.error) entry.reject(new Error(msg.error));
    else entry.resolve(msg.result);
  });

  // Esc leaves full view in the Residuum UI. Keys pressed inside the artifact
  // never reach the page around it, so forward Esc unless the artifact used it.
  // Listening on window runs after the artifact's own handlers.
  window.addEventListener("keydown", (event) => {
    if (embedded && event.key === "Escape" && !event.defaultPrevented) post({ kind: "escape" });
  });

  window.residuum = Object.freeze({
    embedded,
    artifact: __RESIDUUM_ARTIFACT__,
    version: __RESIDUUM_VERSION__,
    features: Object.freeze(__RESIDUUM_FEATURES__.slice()),
    fetch: fetchVia,
    ask,
    on,
    state: Object.freeze({ get: stateGet, set: stateSet }),
    sessions: Object.freeze({ start: startSession }),
  });
})();
