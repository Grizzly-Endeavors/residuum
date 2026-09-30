# Work with more than one agent

One Residuum install can host as many agents as you like. Each has its own identity, memory, model settings and chat integrations, and they share your profile, the team's rules, the knowledge wiki, the workbench and team skills. This guide covers creating agents, moving between them, letting them hand work to each other, and removing one. How the pieces work is described in [Hub](../systems-usage/hub.md), [Team directory](../systems-usage/team-directory.md) and [Teammates](../systems-usage/background-tasks.md#teammates).

The agent you create during setup is your first agent. There is no lead agent: all agents are peers, and you can talk to whichever suits the job.

## Create an agent

Pick a short name of 1 to 24 lowercase letters, digits and hyphens, such as `research-buddy`. The name is permanent: it is the agent's directory, its address for teammates, and its A2A address. `hub`, `team` and `agents` are taken.

- **From the web UI**: choose **Team** in the agent switcher, then fill in **Create an agent**. Give it a name, optionally a description of what it is for, choose which agent to **copy model settings from**, and choose its A2A visibility.
- **From a terminal**: `residuum agent create research-buddy --description "Keeps my reading list and summarizes new papers each morning."`. Add `--models-from <agent>` to say whose model settings to copy (it is required when more than one agent is running), and `--public` to make the agent's card visible to other agents. Residuum must be running.
- **By asking an agent**: tell any agent something like "create an agent called research-buddy that keeps my reading list." It uses its `agent_create` tool. The new agent copies that agent's model settings and A2A visibility. You get a notice when it happens.

The description is optional but useful. The new agent receives it as its first message and turns it into notes in its own `SOUL.md` and a role page in the shared wiki, so it starts knowing what it is for. A new agent skips the first-run interview.

If the new agent can't start (a bad model setting, say), it is still created. It shows as failed in the team view with the reason, and you can fix its settings and start it.

## Switch between agents

The agent switcher lists every agent with a dot for its state and a marker when it is working or has replies you haven't seen. Select one to open its chat, sessions, workspace and settings. Each agent's URL carries its name, so you can bookmark one. **Team** opens the team view and the team-wide pages: the workbench, team files and hub settings.

The team view is also where you start, stop and restart an agent, turn **Start automatically** on or off, and see what went wrong with a failed one. From a terminal, `residuum agent list` shows the same states, and `residuum agent start|stop|restart <name>` controls them. A stopped agent does nothing and receives nothing until you start it.

## Hand work between agents

Agents message each other directly, inside Residuum, using the address `agent:<name>`. You don't need to write addresses yourself: ask an agent in plain words, for example "ask research-buddy to find three papers on this topic". The agent knows its teammates, their states and their roles, and sends the message with `message_agent`. Delivery is fire and forget. The teammate replies with a message of its own when it has something to report, and its reply appears in the sender's conversation.

- The receiving agent sees who sent the request and can reply to that sender.
- A stopped or failed teammate can't take work. The sender gets an error saying so, and nothing is queued. Start the agent, then ask again.
- Long chains of agents passing work back and forth are cut off by a hop limit, and the agents are told why.

## What is shared and what isn't

| Shared by every agent (`team/`) | Belongs to one agent |
|---|---|
| Your profile (`team/USER.md`) and the team's rules (`team/AGENTS.md`) | Identity and personality (`SOUL.md`) |
| The knowledge wiki, including one role page per agent | Memory, sessions and inbox |
| The workbench | Model settings and provider choice |
| Team skills | Its own skills, pulses and scheduled actions |
| Secrets, agent keys and A2A caller keys | Discord, Telegram and Teams settings and MCP servers |

Agents see the shared folder under the `team/` prefix, for example `team/wiki/people/sam.md`. If two agents, or an agent and you, edit the same team file at once, the later save is refused and the agent is told who changed it, so it can read the file again and reapply its edit. Nothing is silently overwritten.

## Delete and restore an agent

Delete from the **Team** view (with a confirmation), with `residuum agent delete <name>`, or by asking any agent to delete one. Deleting stops the agent, saves a checkpoint of its directory, removes it, and removes its role page from the wiki. The checkpoint id is shown, and checkpoints are never pruned, so nothing is lost. An agent asked to delete itself does its last work first, because the delete takes effect as soon as it stops.

To bring one back, create a new agent with the same name. It uses the same checkpoint history, so the deleted agent's checkpoints are listed in its Checkpoints view and you can restore its files from there.

## Choose who else can reach an agent

Agents in your team never need A2A to talk to each other. A2A is for agents outside your install, including your own other installs. Each agent has its own visibility, set in the team view under **A2A card**, in the agent's **Settings → A2A**, or with `visibility` under `[a2a]` in its `config/config.toml`:

- **Private** (the default for agents you create): callers with no key, and no relationship to your other installs, get nothing, not even the agent's card.
- **Public**: anyone can read the card. Sending tasks still needs a caller key.

A caller key works for every agent in the hub, private ones included. With Residuum Cloud connected, each running agent is reachable at `{origin}/a2a/{instance}/{agent}`. See [Connect agents with A2A](a2a-setup.md).
