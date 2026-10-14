# Migrating to Checked Background Models

A pulse's or scheduled action's `model_tier` takes a tier (`small`, `medium`, `large`) or one explicit model as `provider/model`, and Residuum checks it before running anything. A value it can't use is no longer quietly swapped for `small` (pulses) or `medium` (actions). Earlier versions ran a misspelled or wrongly capitalized tier, such as `Small` or `lagre`, on the default tier without saying so. That pulse or action now stops and tells you why.

## Pulses with an invalid `model_tier`

A pulse whose `model_tier` is neither a tier nor a `provider/model` id fails to load. Every other pulse in `HEARTBEAT.yml` keeps running. You get a notice naming the pulse and the bad value, and the Schedule place lists the problem on that pulse.

A pulse naming a `provider/model` whose provider isn't in `providers.toml` (and isn't a provider type such as `anthropic`) loads, but is skipped and reported the same way until the provider exists.

**What to do:** fix the value in `HEARTBEAT.yml`. Write `small`, `medium`, or `large` in lowercase, write a `provider/model` id, or delete the line to run on `small`. The pulse runs again on its next scheduled tick, with no restart needed.

## Stored scheduled actions with an invalid `model_tier`

`schedule_action` checks the value when an action is scheduled. An action stored with an invalid value before that check existed is **held**: it stays in `scheduled_actions.json` and doesn't run, even once it's due. You get one notice naming it, the Schedule place shows why it's held, and the agent's `list_actions` marks it held with the reason.

**What to do:**
- **A value that isn't a tier or a model id (for example `fast`):** cancel the action (Cancel in the Schedule place, or ask the agent) and schedule it again with a tier or a `provider/model` id. Nothing else makes it run.
- **A `provider/model` id whose provider isn't configured:** add the provider to `providers.toml`. The action runs on the next 30-second tick after the reload. Or cancel it and schedule it again with another model.

See [background-tasks.md](../systems-usage/background-tasks.md#models) for how tiers and explicit models resolve.
