# I/O & Routing — Design

## Overview

Residuum's communication architecture is built on two independent layers: **I/O** (how messages move) and **Routing** (who cares about what). These are separate concerns that compose cleanly — I/O endpoints know how to send and receive, routing policy decides when and where.

The agent is a first-class participant in routing decisions. Static routing rules handle the common case, but the agent can override, escalate, or redirect at any time. A background task result that looks routine gets filed quietly; one that looks urgent gets pushed to the user on Discord with a notification. The agent decides, not the config.

---

## I/O Layer

The I/O layer manages endpoints — anything the system can send to or receive from. Every endpoint has a name, a direction, and a transport.

### Endpoint Types

**Interactive endpoints** support bidirectional, real-time conversation. A user sends a message, the agent responds on the same endpoint, the user replies — a natural thread. Interactive endpoints can also be *initiated* by the agent: the user was last seen on the web UI, but the agent decides to open a thread on Discord because it's more likely to get their attention.

Examples: WebSocket (web UI, CLI), Discord DM, Telegram chat.

Interactive endpoints have two capabilities:
- **Receive**: accept inbound messages from a user or external system.
- **Reply**: send responses correlated to an inbound message.
- **Initiate**: send an outbound message unprompted, starting a new thread.

Not all interactive endpoints support initiation. A WebSocket client that disconnects can't be reached. A Discord DM channel can always be initiated if the user has previously interacted.

**Non-interactive endpoints** are unidirectional. They either accept input or produce output, never both in the same conversation.

- **Input-only**: Inbox (user drops something for the agent to look at later), inbound webhooks (external systems pushing events).
- **Output-only**: Push notifications (ntfy), outbound webhooks (HTTP POST to an external service).

### Endpoint Registry

All endpoints — interactive and non-interactive — live in a single registry. Each endpoint has:

- **Name**: unique identifier (e.g., `discord`, `telegram`, `web`, `ntfy-phone`, `inbox`).
- **Direction**: `interactive`, `input-only`, or `output-only`.
- **Status**: `available` (can send/receive now), `reachable` (can initiate but no active session), or `offline` (not connected, can't be reached).
- **Capabilities**: which operations the endpoint supports (receive, reply, initiate, deliver).

The registry is dynamic. WebSocket endpoints come and go as clients connect and disconnect. Discord is reachable as long as the bot is authenticated. The agent can query the registry to discover what's available before deciding where to send something.

### Agent as Endpoint Consumer

The agent itself is an I/O consumer, not an endpoint. It receives input from the routing layer (user messages, background results, external events) and produces output that the routing layer delivers to endpoints. The agent doesn't interact with endpoints directly — it expresses intent ("send this to the user on Discord", "push-notify this") and the I/O layer handles transport.

---

## Routing Layer

The routing layer answers one question: **who cares about this, and how much?**

When something happens — a user message arrives, a background task completes, a pulse finds something, an external webhook fires — the routing layer determines:

1. **Does the agent care?** And with what urgency:
   - **Immediate**: interrupt the current turn if busy, start a turn if idle.
   - **Passive**: include in the agent's context on the next natural turn.
   - **None**: the agent doesn't need to see this.

2. **Does the user care?** And through which endpoints:
   - **Reply**: respond on the same endpoint the user messaged from (default for interactive conversations).
   - **Deliver**: send to one or more specific endpoints (push notification, inbox, Discord DM).
   - **None**: the user doesn't need to see this.

3. **Does an external system care?** Fire outbound webhooks, update external services, etc.

### Default Routing

Most routing is straightforward and doesn't require agent judgment:

- **User message on an interactive endpoint** → agent cares (immediate), reply on the same endpoint.
- **Inbox item added by user** → agent cares (passive), no user delivery needed (user already knows).
- **Inbound webhook** → agent cares (urgency depends on config), no reply (fire-and-forget input).

These defaults are handled automatically. The agent doesn't need to make a decision for the common case.

### Agent-Driven Routing

The interesting case is when the agent is in the loop. Background task results, pulse findings, and proactive notifications all flow through the agent, which decides what to do:

- A pulse checks email and finds nothing. The agent sees the HEARTBEAT_OK, does nothing. No user notification.
- A pulse checks email and finds something urgent. The agent evaluates the content, decides the user needs to know now, and initiates on Discord + sends a push notification.
- A background subagent finishes a code review. The agent reads the summary, decides it's not urgent, and drops a note in the web UI for the user to see next time they open it.
- An external webhook reports a deploy failure. The agent sees it, decides this is critical, and reaches the user through every available channel.

The agent has access to:
- The endpoint registry (what's available, what's reachable).
- The user's notification preferences (quiet hours, preferred channels, escalation rules).
- The content of the event (to judge urgency and relevance).

This means routing tools replace static routing config for background tasks. Instead of declaring `channels: [agent_feed, ntfy]` at task spawn time, the agent receives the result and decides what to do with it. The spawn config only specifies what the agent's involvement should be — "give me the result" — not what happens after.

### Notification Preferences

User-managed preferences constrain the agent's routing decisions:

- **Quiet hours**: don't push-notify between midnight and 7am, queue for inbox instead.
- **Channel preferences**: prefer Discord for urgent, inbox for routine.
- **Escalation rules**: if the agent can't reach the user on the preferred channel, fall back to X.

These are advisory inputs to the agent, not hard routing rules. The agent respects them but can override in genuine emergencies (deploy down, security alert, etc.). The preferences file is user-edited, not agent-edited.

---

## How It Composes

### User asks a question on Discord

1. Discord endpoint receives message, delivers to agent (immediate).
2. Agent processes, produces response.
3. Response routes back to Discord (reply on same endpoint).

### Agent wants to proactively reach the user

1. Agent decides the user should know something.
2. Agent queries endpoint registry — Discord is reachable, web UI has an active session.
3. Agent evaluates preferences — user prefers Discord for proactive messages.
4. Agent initiates on Discord, optionally also delivers to web UI.

### Background task completes

1. Subagent finishes, result delivered to agent (passive — agent sees it on next turn, or immediate if the task was flagged urgent).
2. Agent evaluates the result content.
3. Agent decides: routine → no action. Important → deliver to user via preferred endpoints.

### Pulse finds something

1. Pulse subagent evaluates, produces a finding (not HEARTBEAT_OK).
2. Result delivered to agent.
3. Agent reads the finding, judges urgency, and routes accordingly.

### User adds to inbox

1. User adds an item via the inbox UI or API.
2. Inbox endpoint delivers to agent (passive — agent sees it next turn).
3. Agent triages: responds when convenient, or flags for follow-up.

### External webhook fires

1. Webhook endpoint receives POST, delivers to agent (urgency per webhook config).
2. Agent evaluates payload and routes any user-facing output to appropriate endpoints.

---

## Design Principles

1. **I/O is transport, routing is policy.** Endpoints don't decide who sees what. They move bytes. Routing decides who cares and tells I/O where to deliver.

2. **The agent is in the routing loop.** Static rules handle the obvious cases (reply where you were asked). For everything else — background results, proactive notifications, escalations — the agent makes the call. This is what makes it an agent, not a pipeline.

3. **Endpoints are a flat registry.** No split between "interfaces" and "notification channels." Discord is an endpoint. Ntfy is an endpoint. Inbox is an endpoint. They have different capabilities, but they live in one place and are discoverable by the agent.

4. **Interactive endpoints can be initiated.** The agent isn't limited to replying. It can start conversations on any reachable interactive endpoint. This is how proactive communication works.

5. **Preferences, not rules.** User notification preferences guide the agent but don't cage it. Quiet hours mean "prefer not to" — a production outage still gets through.

6. **Background tasks deliver to the agent, not to the user.** The subagent's job is to do the work and report back. The main agent's job is to decide what the user needs to know. Routing is not the subagent's concern.
