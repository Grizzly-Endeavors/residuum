# Connect agents with A2A

A2A (the Agent2Agent protocol) lets agents hand work to each other. With it, another agent can delegate a task to yours and get the result back, your agent can delegate to other agents, and your own Residuum instances can work together. How the system behaves is described in [A2A](../systems-usage/a2a.md). This guide covers setting it up.

A2A is on by default. The steps below are about who can reach your agent, and which agents your agent can reach.

The settings below are in **Settings**: the gear at the bottom of the left-hand rail, or **Settings** in the bottom bar on a phone. A picker at the top, **Settings for**, chooses what you are changing: **All agents** (the install's own settings) or one agent. A path such as **Settings → (agent) → Advanced → Agent-to-agent** means: open Settings, choose the agent, then pick **Agent-to-agent** under the **Advanced** heading. Edits to the install-wide listener are held until you choose **Save changes**; keys and an agent's visibility apply at once.

## Your agent's address

Every agent in your team has its own address, shown in **Settings → (agent) → Advanced → Agent-to-agent → Status** (with a **Copy address** button; the agent has to be running to show it). Which address you get depends on how your install is reachable:

- **Through the Residuum relay**: once Residuum Cloud is connected, each running agent is reachable at `{origin}/a2a/{instance}/{agent name}`, where `{origin}` and `{instance}` are your relay's address and this install's name. Your other installs find it automatically. Anyone else needs a caller key. A stopped or failed agent is not listed and answers `404` until it runs again.
- **On the same machine or network**: `http://<bind>:7702/agents/<agent name>`.
- **Through your own tunnel or reverse proxy**: point it at the A2A port (`7702` by default), never at the gateway port (`7700`), which serves the settings API without a login. Then enter the tunnel's URL as **Your own address** in **Settings → All agents → Advanced → Agent-to-agent**, and choose **Save changes**, or set it in `hub/config.toml`:

  ```toml
  [a2a]
  public_url = "https://agent.example.com"
  ```

  Each agent's address is then `https://agent.example.com/agents/<agent name>`.

Other agents find your agent from its Agent Card at `<address>/.well-known/agent-card.json`.

## Choose what your agent advertises

Each agent's Agent Card lives in its own directory at `config/agent-card.json`. Edit it with **Edit agent-card.json** under **Its card** in **Settings → (agent) → Advanced → Agent-to-agent**, which opens the file in the agent's **Files**, or ask your agent to edit it. You can set:

- **`name`** and **`description`**: how other agents see yours.
- **`skills`**: what it offers. A skill whose `id` matches one of the agent's skills runs with that skill when a caller asks for it.

Changes take effect as soon as the file is saved. If an edit is invalid, the last valid card keeps being served, and **Settings → (agent) → Advanced → Agent-to-agent → Status** shows what's wrong under "Its card has a problem."

## Let another agent in

Every caller needs a key. Keys belong to the whole hub, so a key reaches every agent in your team, private ones included. Create one per caller you want to allow, so you can revoke each separately:

- In **Settings → All agents → Advanced → Agent-to-agent → Caller keys**, choose **Add a caller key**, give it a name and a description of who it is for, and choose **Create key**, or
- run `residuum a2a keys create <name>` in a terminal.

The key is shown once, with a **Copy** button, until you choose **Done**. Give it to the other agent's operator, who configures their agent to send it as `Authorization: Bearer <key>`. Revoke it from the same list, or with `residuum a2a keys revoke <name>`; a revoke offers **Undo** on the toast that follows.

Each caller's conversations with your agent run as their own sessions, which you can follow in your agent's Activity. They never reach your main chat.

### Public or private

- **Public** (the default): anyone can read your Agent Card, but only callers with a key can send tasks.
- **Private**: without a key, and without being one of your own other installs, your agent looks like it doesn't exist. Its card is hidden. A caller with a valid key sees it normally.

Visibility is per agent. Switch it with **Who can find (agent)** in **Settings → (agent) → Advanced → Agent-to-agent**, which applies at once, or with `visibility = "private"` under `[a2a]` in the agent's own `config/config.toml`.

## Let your agent reach other agents

List the agents yours may delegate to in `config/a2a.json`. Edit it with **Edit a2a.json** under **Remote agents** in **Settings → (agent) → Advanced → Agent-to-agent** (then **Save a2a.json**), or ask your agent to add an entry:

```json
{
  "agents": {
    "research": {
      "url": "https://research.example.com/a2a/main",
      "headers": { "Authorization": "Bearer ${agent-key:research_a2a}" }
    }
  }
}
```

Store the key the other agent gave you as an agent key first, in **Settings → All agents → Saved keys** (**Add a key**) or with `residuum agent-keys set research_a2a`, so it never sits in the file itself. Your agent then sees `a2a:research` in `list_agents`, can send it work with `message_agent`, and gets the reply back as a message when the task finishes.

## Link your own instances

If you run several Residuum installs on the same relay account (a laptop and a server, for example), they find and trust each other automatically once their tunnels are connected. You don't need keys or `config/a2a.json` entries for them. Each agent of another install appears as `a2a:<instance>/<agent>`, marked "(your instance)" in `list_agents`. The agents inside one install are teammates and message each other directly, so they never appear as siblings.

## Check it works

- **Settings → (agent) → Advanced → Agent-to-agent → Status** should show A2A as on, with the agent's address and no problems. If it says nothing is answering on the listener's port, restart the agent.
- Under **Remote agents**, each configured agent should show as **Reachable**, with its skills.
- A task your agent sends shows in its **Activity** under **Running now**, with **Stop task** beside it.
