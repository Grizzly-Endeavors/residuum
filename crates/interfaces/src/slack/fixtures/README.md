# Slack wire fixtures — assumed-until-probed

Every fixture in this directory was authored from Slack's published documentation
(Socket Mode and Events API), NOT captured from a live workspace. They are marked
**assumed-until-probed**: at first live deployment, capture real envelopes/events
and replace these files byte-for-byte, then re-run the test suite. Until then,
treat any wire-format bug as possibly ours, not Slack's.

Envelope shapes follow <https://docs.slack.dev/apis/events-api/using-socket-mode>
(§2 `hello`, §3 `disconnect`, §4 "Receive events", "Using interactive features"):
an envelope is `{payload, envelope_id, type, accepts_response_payload}` — there is
**no top-level `response_url`**. Per §5 "Acknowledge events", the ack is a text
frame `{"envelope_id": "<id>"}` sent back over the same websocket.

History: the first fixtures carried an invented top-level `response_url` and the
adapter acked by HTTP POST to it. Live Slack never sends that field, so nothing
was ever acked and Slack retried every event under a fresh `envelope_id`
(duplicate agent turns, PR #498 live probe 2026-10-12). Build to the docs page,
not to memory.

Fixture `envelope_id`s are short readable stand-ins (`env-evt-1`) for Slack's
UUIDs; the slash-command payload in `envelope-unknown.json` is the docs example
with ids anonymized (its `response_url` lives *inside* the payload, as documented).
