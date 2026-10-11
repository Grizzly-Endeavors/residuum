# Auto Mode

Auto Mode checks each of an agent's tool calls against rules written in plain language before the call runs. The rules say what the agent isn't allowed to do ("Pushing to the main branch", "Sending email to anyone but me"), with optional exceptions ("Deleting files under tmp/"). The [decision model](system-one.md) judges each call against them. A call that matches a rule and no exception is skipped, and the agent is told which rule it hit, so it can take another approach or ask the user. Auto Mode is off by default and set per agent.

## Configuration

In the agent's `config.toml`, or in Settings → (agent) → Auto Mode:

```toml
[auto_mode]
enabled = true
deny = ["Pushing to the main branch", "Deleting files outside the workspace"]
allow = ["Deleting files under tmp/"]
threshold = 0.5
```

- `enabled` turns checking on. With no `deny` rules nothing is checked, and an enabled section without any raises a load notice saying so.
- `deny` lists what the agent isn't allowed to do. `allow` lists exceptions to them. Blank rules are dropped.
- `threshold` is the probability, above 0 and at most 1, at which a rule counts as matching (default `0.5`). A value outside that range falls back to the default with a notice.

The section hot-reloads: the main agent and every session it runs share one Auto Mode, so a saved change applies to the next tool call anywhere in that agent.

## How a call is checked

Every tool call the turn loop dispatches is checked: built-in tools and tool-server (MCP) tools, in the main agent's turns and in every session, pulse, scheduled action, webhook and artifact turn. Each call is one request to the decision model. Its state holds the agent's name, the tool's name, its arguments (cut at 8,000 characters for a very large call), the tool server when there is one, and the text of the latest user message (cut at 2,000 characters), so a rule can refer to what the user asked for. Each rule is one yes/no question, "Would carrying out the tool call in `call` do what `rule` describes?", and every rule is answered in parallel within that one request.

The call is blocked when at least one deny rule's probability is at or above the threshold and no allow rule's is. An allow rule at or above the threshold lets the call run even when a deny rule matched.

- **Blocked:** the call doesn't run. Its result, marked as an error, reads: Auto Mode blocked this call, so it did not run: it matches the rule "…" (93% sure). Try a different approach that the rule allows, or ask the user to do it or to change the rule. The turn continues. An `info` log records the tool, rule, probability and input tokens.
- **Allowed:** the call runs normally. A `debug` log records the verdict, any exception that applied, and the input tokens.
- **Unchecked:** when the decision model can't answer (none is set up, it's unreachable, it refused the request, or an answer was missing), the call runs without a check. There is no per-call warning; the decision model's outage shows once on Home until it answers again (see [Decision model](system-one.md#status-and-outages)), and Auto Mode's settings show it too.

## What the user sees

Each tool result frame carries the verdict as `auto_mode` `{ decision, rule, probability, reason, input_tokens }` when Auto Mode checked the call: `tool_result` on the agent's WebSocket, and `session_tool_result` for sessions and in the hub's session relay. The activity feed shows a blocked call's status as "Blocked by Auto Mode" instead of "Failed". A step's details open with one line for every checked call: which rule blocked it and how sure the model was, which exception let it through, that no rule matched, or why it couldn't be checked. Chat history doesn't keep the verdict, so a reloaded conversation shows a blocked call as a failed one, with its result text still naming the rule.

## Stopping it

Turning off `enabled`, or removing the rules, takes effect on the next tool call. A blocked call changes nothing, so there is nothing to undo. The turn's Stop control works the same as without Auto Mode.
