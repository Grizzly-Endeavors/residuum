# I/O & Routing — Design

## Overview

Residuum's communication architecture is built on two layers: **I/O** (how messages move) and **Routing** (who cares about what).

Input routing is **user-managed** — the user decides how external events reach the agent. Output routing is **agent-managed** — the agent (or its subagents) decides what the user needs to know and how to reach them.

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

All endpoints live in a single registry, discoverable by the agent. The registry is dynamic — WebSocket clients come and go, Discord is reachable as long as the bot is authenticated. The agent queries it when deciding where to send output.

---

## Routing

### Input Routing

**Interactive messages** need no routing configuration. The message/response contract is inherent.

**Non-interactive inputs** (webhooks, external events) have two user-configured routes:

- **Inbox**: the event lands in the agent's inbox for passive triage.
- **Subagent**: the event triggers a dedicated subagent that handles it autonomously — evaluates, decides, and routes output without involving the main agent.

The user configures which route each non-interactive input takes because they understand their own workflow and what deserves active handling vs. passive awareness.

### Output Routing

Output routing is agent-managed. When the agent (or a subagent) has something to communicate, it queries the endpoint registry, evaluates what the user needs to know, and delivers through the appropriate endpoints.

This applies uniformly: the main agent deciding to proactively message the user, a subagent handling a webhook event and push-notifying about a failed deploy, or the pulse subagent triaging a finding and reaching out on Discord. The LLM doing the work makes the routing call.

### Background Task Results

When the main agent spawns a subagent, it declares one of two result modes:

- **Wake** (default): the result comes back to the main agent. Agent wakes if idle, gets the result injected if mid-turn. The agent evaluates and decides what to do.
- **Inbox**: the result lands in the agent's inbox for deferred triage. No wake, no injection. The agent sees it when it next checks its inbox — confirms success or investigates failure.

The agent does not declare output routing at spawn time. That decision happens after seeing the result (wake mode) or not at all (inbox mode — the subagent already handled it, the inbox entry is just a receipt).

### Pulse Results

Pulse subagents evaluate a check and produce a finding. If the finding is HEARTBEAT_OK, it's silently logged. If substantive, the pulse subagent handles output routing autonomously — same as any subagent triggered by a non-interactive input. The main agent is not involved. (Needs code level system to ensure non-OK results are actually routed)

---

## How It Composes

**User asks a question on Discord** — Agent receives it, responds on Discord. Inherent contract.

**Agent wants to proactively reach the user** — Agent queries registry, picks the right endpoint, initiates.

**Subagent spawned in wake mode** — Result returns to main agent. Agent evaluates, decides whether and how to notify the user.

**Subagent spawned in inbox mode** — Result lands in agent's inbox. Agent triages later.

**Pulse finds something** — Pulse subagent evaluates the finding and handles routing autonomously (notify user, push, etc.).

**User adds to inbox** — Item sits until the agent triages. Agent decides how to respond.

**Webhook fires, routed to subagent** — Subagent evaluates the event and handles output routing. Main agent isn't involved.

**Webhook fires, routed to inbox** — Event lands in agent's inbox for passive triage.

---

## Design Principles

1. **I/O is transport, routing is policy.** Endpoints move messages. Routing decides who cares.

2. **Input routing is user-managed, output routing is agent-managed.** Users decide how the world reaches the agent. The agent decides how it reaches back out.

3. **Interactive endpoints have an inherent contract.** No configuration needed.

4. **Endpoints are a flat registry.** No split between "interfaces" and "notification channels." They're all endpoints with different capabilities.

5. **The inbox is the agent's input queue.** Not a delivery destination.

6. **Background tasks deliver to the agent, not to the user.** Subagents report results. The receiving LLM (main agent or autonomous subagent) decides what the user needs to know.
