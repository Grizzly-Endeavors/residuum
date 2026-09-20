# I/O & Routing — Design

## Overview

Residuum's communication architecture is built on a **pub/sub message bus**. Systems publish events, other systems subscribe to the events they care about. Endpoints, the agent, subagents, and the inbox are all participants on the same bus.

Input routing is **user-managed** — the user decides how external events reach the agent. Output routing is **agent-managed** — the agent (or its subagents) decides what the user needs to know and how to reach them.

---

## The Bus

All communication flows through a central pub/sub bus. Every participant is a publisher, a subscriber, or both. There is no distinction between "interfaces" and "notification channels" — they're all bus participants with different roles.

### Participants

**Interactive Endpoints** (WebSocket, Discord, Telegram) — publish inbound user messages, subscribe to outbound agent responses. Either party can initiate. The message/response contract is inherent to interactive endpoints and needs no configuration.

**Non-Interactive Input Endpoints** (inbound webhooks) — publish events onto the bus. Don't subscribe to anything. Fire-and-forget.

**Non-Interactive Output Endpoints** (ntfy, outbound webhooks) — subscribe to messages routed to them. Don't publish. Deliver-and-forget.

**Main Agent** — subscribes to interactive messages (always immediate) and to subagent results in wake mode. Publishes responses on interactive endpoints and output routing decisions to any endpoint.

**Subagents** — subscribe to events they're configured to handle (pulse findings, webhook events, spawned task work). Publish results back to the bus — either as wake events (main agent subscribes) or inbox items. Autonomous subagents also publish output routing decisions directly to endpoints.

**Inbox** — subscribes to inbox-routed events (user-added items, inbox-mode subagent results, inbox-routed webhook events). Publishes nothing — it's a passive store the agent reads on its own schedule.

**Pulse Scheduler** — publishes pulse triggers on schedule. Doesn't subscribe to anything.

### Routing as Subscription Configuration

Input routing is subscription wiring. When the user configures "this webhook routes to a subagent," they're subscribing a subagent to that webhook's topic. When they configure "this webhook routes to inbox," they're subscribing the inbox to it. Interactive endpoints are pre-wired — the main agent always subscribes to interactive messages.

Output routing is the agent (or subagent) choosing which topics to publish to at runtime. The endpoint registry is the list of available topics the agent can target.

---

## I/O Layer

### Interactive Endpoints

Interactive endpoints support bidirectional conversation. Either party can initiate — a user messages the agent, or the agent reaches out proactively. Responses flow on the same endpoint. This is an inherent contract, not something that needs configuration.

Examples: WebSocket (web UI, CLI), Discord DM, Telegram chat.

### Non-Interactive Endpoints

Non-interactive endpoints are unidirectional.

- **Input-only**: the agent's inbox, inbound webhooks.
- **Output-only**: push notifications (ntfy), outbound webhooks.

### The Inbox

The inbox is the agent's low-priority input queue. The user is the producer, the agent is the consumer. Users add items when they want the agent to look at something non-urgently. The inbox doesn't wake the agent — items sit there until the agent triages them naturally.

The inbox is not a delivery destination for agent output.

### Endpoint Registry

The endpoint registry is the bus's topic directory. All endpoints are listed, the agent can query what's available and reachable when making output routing decisions.

---

## Routing

### Input Routing

**Interactive messages** need no routing configuration. The main agent subscribes to all interactive endpoint topics by default.

**Non-interactive inputs** (webhooks, external events) have two user-configured subscription targets:

- **Inbox**: the event is published to the inbox topic. Agent triages later.
- **Subagent**: the event is published to a subagent topic. The subagent handles it autonomously — evaluates, decides, and publishes output to whatever endpoints are appropriate. Main agent is not involved.

### Output Routing

Output routing is agent-managed. When the agent (or a subagent) has something to communicate, it publishes to the appropriate endpoint topics. The LLM doing the work makes the routing call — which endpoints, how many, based on content and what's available.

### Background Task Results

When the main agent spawns a subagent, it declares one of two result modes:

- **Wake** (default): the subagent publishes its result to the main agent's wake topic. Agent wakes if idle, gets the result injected if mid-turn. The agent evaluates and decides what to do.
- **Inbox**: the subagent publishes its result to the inbox topic. No wake, no injection. The agent sees it when it next triages its inbox.

### Pulse Results

Pulse subagents evaluate a check and produce a finding. If the finding is HEARTBEAT_OK, it's silently logged — nothing is published. If substantive, the pulse subagent handles output routing autonomously — publishing to whatever endpoint topics are appropriate. The main agent is not involved.

(Needs a code-level mechanism to ensure non-OK results are actually routed.)

---

## How It Composes

**User asks a question on Discord** — Discord endpoint publishes message. Main agent (subscriber) receives it, processes, publishes response to Discord topic.

**Agent wants to proactively reach the user** — Agent queries endpoint registry, publishes to the appropriate endpoint topic(s).

**Subagent spawned in wake mode** — Subagent completes, publishes result to agent wake topic. Main agent receives, evaluates, publishes output to endpoints as needed.

**Subagent spawned in inbox mode** — Subagent completes, publishes result to inbox topic. Agent triages later.

**Pulse finds something** — Pulse subagent publishes output directly to endpoint topics (Discord, ntfy, etc.) based on its own judgment.

**User adds to inbox** — Item published to inbox topic. Agent triages on next natural cycle.

**Webhook fires, routed to subagent** — Webhook publishes event. Configured subagent (subscriber) picks it up, evaluates, publishes output to endpoints.

**Webhook fires, routed to inbox** — Webhook publishes event. Inbox (subscriber) stores it. Agent triages later.

---

## Design Principles

1. **Pub/sub is the backbone.** All communication is publish/subscribe on a shared bus. No point-to-point wiring, no special cases.

2. **Input routing is user-managed, output routing is agent-managed.** Users configure subscriptions for inbound events. The agent decides where to publish outbound messages.

3. **Interactive endpoints have an inherent contract.** No configuration needed — the main agent always subscribes.

4. **Everything is a bus participant.** Endpoints, the agent, subagents, the inbox. No split between "interfaces" and "notification channels."

5. **The inbox is the agent's input queue.** Not a delivery destination.

6. **Background tasks deliver to the agent, not to the user.** Subagents publish results. The receiving LLM decides what the user needs to know.
