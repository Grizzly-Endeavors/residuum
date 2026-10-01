<script lang="ts">
  import { hub } from "../../lib/hub.svelte";
  import type { A2aVisibility } from "../../lib/hub-types";
  import { router } from "../../lib/router.svelte";
  import { ALL_SCOPE } from "../../lib/settings-sections";
  import { toast } from "../../lib/toast.svelte";
  import { Button, SegmentedControl, type Choice } from "../../lib/ui";
  import A2aCardPreview from "./A2aCardPreview.svelte";
  import A2aStatus from "./A2aStatus.svelte";
  import RemoteAgents from "./RemoteAgents.svelte";
  import RunningOnly from "./RunningOnly.svelte";
  import { fieldError, type AgentSectionProps } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";

  // Advanced → Agent-to-agent for one agent. Who can find it is set here and
  // nowhere else, and applies at once through the hub, not with Save
  // changes. Whether the install's listener is on is the install's setting,
  // shown here read-only. The status and the card need the agent running;
  // the remote agents are a file, so they stay editable while it is stopped.

  let { scope, section }: AgentSectionProps = $props();

  const agent = $derived(scope.agent);

  const VISIBILITY: readonly Choice<A2aVisibility>[] = [
    { value: "private", label: "Private" },
    { value: "public", label: "Public" },
  ];
  const VISIBILITY_HINTS: Readonly<Record<A2aVisibility, string>> = {
    private: "Agents outside this install need a caller key even to see it.",
    public:
      "Anyone with its address can see what it does. Handing it work still needs a caller key.",
  };

  /** The visibility being applied, shown until the hub answers. */
  let pending = $state<A2aVisibility | null>(null);
  const visibility = $derived(pending ?? hub.agent(agent)?.a2a_visibility ?? "private");

  async function changeVisibility(next: A2aVisibility): Promise<void> {
    const name = agent;
    pending = next;
    const applied = await hub.setVisibility(name, next);
    pending = null;
    if (!applied) return;
    toast.success(
      next === "public"
        ? `${name} is public: anyone with its address can see what it does.`
        : `${name} is private: agents need a caller key even to see it.`,
    );
    // The hub wrote the agent's config.toml; read it again so the raw editor shows it.
    void scope.load();
  }

  function openListener(): void {
    void router.openSettings({ scope: ALL_SCOPE, section: "listener" });
  }

  function openCardFile(): void {
    void router.openPlace(
      { kind: "files", agent },
      { panel: { kind: "file", path: "config/agent-card.json" } },
    );
  }
</script>

<SettingsSection
  {scope}
  {section}
  title="Agent-to-agent"
  lede={`Whether agents outside this install can find ${agent}, and the agents it can hand work to.`}
>
  <div class="groups">
    <section class="group" aria-labelledby="a2a-find">
      <h3 class="group-title" id="a2a-find">Who can find {agent}</h3>
      <SegmentedControl
        label="Who can find {agent}"
        labelHidden
        value={visibility}
        options={VISIBILITY}
        disabled={pending !== null}
        hint={VISIBILITY_HINTS[visibility]}
        error={fieldError(scope, { kind: "config", field: "a2a_visibility" })}
        onchange={(next) => void changeVisibility(next)}
      />
      <div class="listener">
        <p class="note">
          {#if scope.install.a2a_enabled}
            This install's listener is on, so agents elsewhere can reach the agents they can find.
          {:else}
            This install's listener is off, so nothing outside this install can reach {agent}.
          {/if}
        </p>
        <Button size="sm" variant="quiet" onclick={openListener}>Change for all agents</Button>
      </div>
    </section>

    <section class="group" aria-labelledby="a2a-status">
      <h3 class="group-title" id="a2a-status">Status</h3>
      <RunningOnly {agent} subject="its status and address">
        <A2aStatus {agent} />
      </RunningOnly>
    </section>

    <section class="group" aria-labelledby="a2a-remote">
      <h3 class="group-title" id="a2a-remote">Remote agents</h3>
      <p class="note">
        Agents {agent} can hand work to: the ones listed in a2a.json, and the agents of your other installs,
        found through Residuum Cloud.
      </p>
      <RemoteAgents {agent} />
    </section>

    <section class="group" aria-labelledby="a2a-card">
      <h3 class="group-title" id="a2a-card">Its card</h3>
      <p class="note">
        What {agent} shows the agents that reach it. It comes from agent-card.json in its files.
      </p>
      <RunningOnly {agent} subject="its card">
        <A2aCardPreview {agent} />
      </RunningOnly>
      <div>
        <Button icon="edit" onclick={openCardFile}>Edit agent-card.json</Button>
      </div>
    </section>
  </div>
</SettingsSection>

<style>
  .groups {
    display: flex;
    flex-direction: column;
    gap: var(--space-32);
    max-width: 640px;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }

  .group-title {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .note {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .listener {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4) var(--space-12);
  }
</style>
