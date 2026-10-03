# Decision Model (System 1)

A decision model is a fast model that answers typed questions about some text instead of writing text: the probability that a yes/no question is true (a *noul*), which of a set of options fits (a *choice*), or where something sits on an ordered scale (a *score*). TypeSafe calls these System 1 models, and its Jev models are the first. Ollama 0.35 and later serves the same API for decision models that run locally (Nimble, Tev1, Clef and others). Residuum has one decision model per install, shared by every agent. [Auto Mode](auto-mode.md) is its first user.

## Configuration

The decision model is set in `hub/config.toml`, or in Settings → All agents → Decision model:

```toml
[system_one]
provider = "typesafe"         # "typesafe", "ollama", or "other"
model = "jev-latest"
api_key = "secret:system_one" # a literal, "secret:<name>", or "${ENV_VAR}"
# url = "https://..."         # required for "other"
# keep_alive = "5m"           # sent to Ollama only
```

| Provider | Default address | Model | Key |
|----------|-----------------|-------|-----|
| `typesafe` | `https://api.typesafe.ai` | defaults to `jev-latest` | required; falls back to the `TYPESAFE_API_KEY` environment variable |
| `ollama` | `http://localhost:11434` | required (`nimble`, `tev1`, ...) | none |
| `other` | none, `url` is required | required | optional |

`url` overrides the default address for any provider, so an Ollama on another machine works with either `ollama` or `other`. An address ending in `/v1` is accepted; the client adds `/v1/systemone` and `/v1/models` itself. In the settings form, a key typed into the API key field is stored in the encrypted secret store as `system_one` and the file gets the `secret:system_one` reference, the same as other credentials.

A gap in the section is a load notice, not a failed load: no provider, an unknown provider, `other` without a `url`, `ollama` or `other` without a `model`, a TypeSafe provider with no key anywhere, or a `secret:` reference that isn't in the store. A section too incomplete to call (the first four) leaves the hub with no decision model. The section hot-reloads with the rest of `hub/config.toml`, and a changed section replaces the client in place and clears any outage.

## Wire format

Every provider takes `POST {base}/v1/systemone` with `{ model, state, questions }`. `state` is a string or JSON value; `questions` is a map from an ID the caller picks to a typed question with `instructions` (a string or a structured object) and, for choice and score, `criteria`. The answer is `{ model, answers, usage }`, with one answer per question ID and `model` naming the versioned model that answered. `GET {base}/v1/models` lists model names; TypeSafe answers `{ models: [{ name, description }] }` and Ollama answers the OpenAI shape `{ data: [{ id }] }`, and the client reads both.

The client lives in `src/inference/system_one/`. A rate limit (`429`), an overloaded or failing service (`5xx`, including TypeSafe's `529`), a timeout, or a connection failure is retried with exponential backoff using the default retry policy; it logs one `warn` when retries start and one if they run out. Requests time out after 120 seconds, since a local model may still be loading on the first request.

## Status and outages

The hub keeps one status for the decision model: whether it is configured, the provider's name and model, and an outage when there is one. An outage begins with the first call that fails in a way that means the service as a whole can't answer: no decision model is set up, it can't be reached, it timed out or is overloaded after retries, it refused the key, it doesn't have the model, or it answered with something unreadable. A request the service rejected on its own (for example, one too large to evaluate) is logged at `warn` and leaves the status alone. An unconfigured decision model that nothing has asked for has no outage.

While an outage lasts, a recovery check asks the configured service one tiny question every 60 seconds (logged at `trace`), so the outage clears when the service is back even if nothing else is asking. Any successful call, or a config change, also clears it. The status changes are logged once each: a `warn` when the service becomes unavailable and an `info` when it answers again.

The hub WebSocket sends the status as `system_one_status` right after `hub_boot`, and again whenever it changes (see [Hub HTTP](hub-http.md#hub-websocket)). While there is an outage, Home's Needs you list shows one item, "Auto Mode can't check tool calls", with the plain-language reason, when it began, and a button to the Decision model settings. It goes away when the outage ends. The Decision model settings section shows the same status as a badge (Not set up, Set up, Not answering) and the outage's reason.

## Trying a configuration

The Decision model settings section has two buttons that use the values in the form, saved or not:

- **Show available models** calls `POST /api/hub/system-one/models` and fills the model box's suggestions. The box still accepts any model name, since TypeSafe's list holds only aliases and versioned IDs are accepted too.
- **Test connection** calls `POST /api/hub/system-one/test`, which asks the model whether "Hello there!" is a greeting and reports what happened in plain words. It doesn't touch the running service or its status.

Both resolve a `secret:` key against the hub's secret store.
