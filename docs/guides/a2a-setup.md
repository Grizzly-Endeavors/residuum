# Connect agents with A2A

A2A (the Agent2Agent protocol) lets agents hand work to each other. With it, another agent can delegate a task to yours and get the result back, your agent can delegate to other agents, and your own Residuum instances can work together. How the system behaves is described in [A2A](../systems-usage/a2a.md). This guide covers setting it up.

A2A is on by default. The steps below are about who can reach your agent, and which agents your agent can reach.

## Your agent's address

Every agent in your team has its own address on the A2A port, `http://<bind>:7702/agents/<agent name>`, shown in **Settings → A2A → Status**. That address works from the same machine or network. The Residuum relay doesn't carry A2A yet, so the status page says "Reachable locally; remote access through the relay arrives with relay support." To reach an agent from elsewhere, use your own tunnel:

- **Through your own tunnel or reverse proxy**: point it at the A2A port (`7702` by default), never at the gateway port (`7700`), which serves the settings API without a login. Then enter the tunnel's URL as **Your own address** in **Settings → A2A** (under the advanced settings), or set it in `hub/config.toml`:

  ```toml
  [a2a]
  public_url = "https://agent.example.com"
  ```

  Each agent's address is then `https://agent.example.com/agents/<agent name>`.

Other agents find your agent from its Agent Card at `<address>/.well-known/agent-card.json`.

## Choose what your agent advertises

The Agent Card lives in your workspace at `config/agent-card.json`. Edit it from the workspace panel, or ask your agent to edit it. You can set:

- **`name`** and **`description`**: how other agents see yours.
- **`skills`**: what it offers. A skill whose `id` matches one of your workspace skills runs with that skill when a caller asks for it.

Changes take effect as soon as the file is saved. If an edit is invalid, the last valid card keeps being served, and **Settings → A2A** shows what's wrong.

## Let another agent in

Every caller needs a key. Create one per agent you want to allow, so you can revoke each separately:

- In **Settings → A2A → Caller keys**, choose **Create key**, or
- run `residuum a2a keys create <name>` in a terminal.

The key is shown once. Give it to the other agent's operator, who configures their agent to send it as `Authorization: Bearer <key>`. Revoke it from the same page, or with `residuum a2a keys revoke <name>`.

Each caller's conversations with your agent run as their own sessions, which you can follow in the sessions sidebar. They never reach your main chat.

### Public or private

- **Public** (the default): anyone can read your Agent Card, but only callers with a key can send tasks.
- **Private**: without a key, your agent looks like it doesn't exist. Its card is hidden.

Visibility is per agent. Switch it with the **Visibility** setting in **Settings → A2A**, or with `visibility = "private"` under `[a2a]` in the agent's own `config/config.toml`.

## Let your agent reach other agents

List the agents yours may delegate to in `config/a2a.json`. Edit it in **Settings → A2A → Remote agents**, or ask your agent to add an entry:

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

Store the key the other agent gave you as an agent key first, for example with `residuum agent-keys set research_a2a`, so it never sits in the file itself. Your agent then sees `a2a:research` in `list_agents`, can send it work with `message_agent`, and gets the reply back as a message when the task finishes.

## Link your own instances

If you run several Residuum installs on the same relay account (a laptop and a server, for example), they find and trust each other automatically once their tunnels carry A2A. You don't need keys or `config/a2a.json` entries for them. Each install appears to the others as `a2a:<instance>`, marked "(your instance)" in `list_agents`. The agents inside one install are teammates and message each other directly, so they never appear as siblings.

## Check it works

- **Settings → A2A → Status** should show A2A as on, with the agent's address and no problems.
- Under **Remote agents**, each configured agent should show as reachable, with its skills.
