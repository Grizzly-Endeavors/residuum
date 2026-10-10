# Slack Interface — Build Plan (issue #490)

Worktree for feat/slack-interface. Branch staged by DSH-vex at c373b113, pushed to fork.
Spec: Grizzly-Endeavors/residuum#490 (Josh-authored). DM-only v1, one app per agent,
Socket Mode only, conversation-only outbound contract (post-22c0773e), hand-rolled client
per Bear (no Slack SDK). tokio-tungstenite 0.26 already in tree (see src/tunnel/connection.rs
for the house ws-client style).

## Files to create: src/interfaces/slack/
- `mod.rs` — SlackInterface (wiring template: discord/mod.rs:150-231). State Arc,
  ChatStateStore::load(layout.slack_state_json()), BaseSubscribers::new(endpoint "slack"),
  conversations.register, spawn outbound subscriber, shutdown watch.
- `socket.rs` — hand-rolled Socket Mode client:
  POST apps.connections.open (xapp token, plain HTTPS via shared reqwest) → wss URL →
  tokio-tungstenite connect → envelope loop. Envelopes: hello (ignore payload), events_api
  (ACK {envelope_id} within ~3s, then process payload), disconnect (reconnect with
  boot_retry family). Ping/pong per ws protocol.
- `handler.rs` — inbound: events_api payload event.type == "message", channel_type == "im",
  sender != bot id → publish UserMessage (crib telegram/handler.rs post-160 for the exact
  publish path + context struct). DM-only; everything else dropped at envelope boundary.
- `subscriber.rs` — ChatOutbound impl (template: discord/subscriber.rs, 178 lines).
  Replies via chat.postMessage HTTPS (xoxb token) — NOT over the socket. Target = channel id.
  No system notices (contract), no typing indicator in v1.

## Config plumbing (three files + example)
- `src/config/deserialize.rs` — SlackConfigFile beside DiscordConfigFile (~line 417):
  deny_unknown_fields; bot_token: Option<String>, app_token: Option<String> (${ENV_VAR} +
  secret: expansion per token precedent). Field on root file struct (~line 115 area).
- `src/config/resolve/channels.rs` — resolve_slack_config (~line 33 pattern, wire ~288-298).
- `src/config/types.rs` — SlackConfig beside DiscordConfig (~164): bot_token, app_token.
  Redacted Debug. Field on resolved config (~919).
- `src/gateway/event_loop/http.rs` — spawn block after discord's (~161-172):
  SlackInterface::new + spawn_monitored("slack", ...).
- `assets/config.example.toml` — [slack] section doc.
- `src/workspace/layout.rs` — slack_state_json() beside discord_state_json().

## Secrets
Tokens arrive as `secret:slack_vex`-style or literal; follow the token resolution path
telegram uses (resolve/channels.rs reads via secrets store). Keyring names: one pair per
agent (6 agents = 6 apps; free-tier cap 10 integrations, fits).

## Acceptance (from issue)
- DM answered by agent's bot user; no notices reach Slack; hub restart reconnects cleanly;
  config example documents the section; tests cover conversation-only contract + DM filter.
- Live test blocked on Josh: Slack app in Big Bear Hollow (workspace created 12:18 ET 10/10)
  with Socket Mode enabled → need xapp- + xoxb- tokens. Asked in #490 comment.

## Status
- [x] Survey: spawn pattern, config structs, discord run() read end-to-end
- [ ] Config plumbing
- [ ] socket.rs client
- [ ] handler.rs inbound
- [ ] subscriber.rs outbound
- [ ] layout.rs + example docs
- [ ] cargo check + targeted tests
- [ ] PR to upstream (branch → fork → PR; I hold push, but PR is the review path)
