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

  function post(message) {
    window.parent.postMessage({ tag: TAG, ...message }, "*");
  }

  function request(kind, payload) {
    if (!embedded) return Promise.reject(notEmbedded());
    const id = `req-${++nextId}`;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      post({ kind, id, ...payload });
    });
  }

  function encodeBody(body, headers) {
    if (body === undefined || body === null) return null;
    if (typeof body === "string") return body;
    if (Object.getPrototypeOf(body) === Object.prototype || Array.isArray(body)) {
      if (!Object.keys(headers).some((k) => k.toLowerCase() === "content-type")) {
        headers["Content-Type"] = "application/json";
      }
      return JSON.stringify(body);
    }
    throw new TypeError("residuum.fetch body must be a string, a plain object, or an array");
  }

  function fetchVia(path, init = {}) {
    if (typeof path !== "string") {
      return Promise.reject(new TypeError("residuum.fetch path must be a string like '/api/status'"));
    }
    const headers = { ...(init.headers || {}) };
    let body;
    try {
      body = encodeBody(init.body, headers);
    } catch (err) {
      return Promise.reject(err);
    }
    return request("fetch", {
      path,
      method: (init.method || "GET").toUpperCase(),
      headers,
      body,
    }).then(
      (r) => new Response(r.status === 204 || r.status === 304 ? null : r.body, {
        status: r.status,
        statusText: r.statusText,
        headers: r.headers,
      }),
    );
  }

  function send(content) {
    if (typeof content !== "string" || content.trim() === "") {
      return Promise.reject(new TypeError("residuum.send needs a non-empty string"));
    }
    return request("send", { content }).then(() => undefined);
  }

  function on(type, handler) {
    if (typeof handler !== "function") throw new TypeError("residuum.on handler must be a function");
    if (!handlers.has(type)) handlers.set(type, new Set());
    handlers.get(type).add(handler);
    if (embedded && !subscribed) {
      subscribed = true;
      post({ kind: "subscribe" });
    }
    return () => handlers.get(type)?.delete(handler);
  }

  function dispatch(frame) {
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
    send,
    on,
  });
})();
