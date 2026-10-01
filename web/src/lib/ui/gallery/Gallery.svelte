<script lang="ts">
  import {
    Badge,
    Banner,
    Button,
    ConfirmDialog,
    ConfirmHost,
    Dialog,
    Disclosure,
    Drawer,
    EmptyState,
    IconButton,
    Kbd,
    NumberField,
    SecretField,
    SegmentedControl,
    SelectField,
    Sheet,
    Skeleton,
    StatusDot,
    Tabs,
    TextField,
    Toggle,
    confirmations,
    confirmLeave,
    type ButtonVariant,
    type StatusDotState,
  } from "..";
  import { router } from "../../router.svelte";

  // Every primitive in every state, served at /dev/gallery in development and
  // mock builds. Controls are live, so keyboard behavior can be tried here.

  const VARIANTS: readonly { variant: ButtonVariant; label: string }[] = [
    { variant: "primary", label: "Save changes" },
    { variant: "secondary", label: "New agent" },
    { variant: "quiet", label: "Discard" },
    { variant: "danger", label: "Disconnect" },
  ];

  const STATES: readonly { state: StatusDotState; working?: boolean; word: string }[] = [
    { state: "running", word: "Running" },
    { state: "running", working: true, word: "Working" },
    { state: "starting", word: "Starting" },
    { state: "stopping", word: "Stopping" },
    { state: "stopped", word: "Stopped" },
    { state: "failed", word: "Failed" },
  ];

  const PROVIDERS = [
    { value: "anthropic", label: "Anthropic" },
    { value: "openai", label: "OpenAI" },
    { value: "ollama", label: "Ollama (on this computer)" },
  ];

  const THINKING = [
    { value: "off", label: "Off" },
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High" },
  ] as const;

  let name = $state("research-buddy");
  let badName = $state("Research Buddy");
  let notes = $state("");
  let context = $state<number | null>(20);
  let port = $state<number | null>(80);
  let provider = $state("anthropic");
  let unsetProvider = $state("");
  let model = $state("claude-9");
  let others = $state(false);
  let pulses = $state(true);
  let autostart = $state(true);
  let thinking = $state<(typeof THINKING)[number]["value"]>("medium");
  let invalidThinking = $state<(typeof THINKING)[number]["value"]>("high");
  let storedKey = $state("");
  let envKey = $state("");
  let newKey = $state("");
  let replacingKey = $state("sk-live-");
  let settingsOpen = $state(false);
  let jobsOpen = $state(true);
  let inboxTab = $state<"inbox" | "archived" | "later">("inbox");
  let showDismissible = $state(true);

  // Back closes the overlays opened here, without the router reading this
  // page's address as a place.
  router.startForOverlays();

  const AGENTS: readonly { name: string; role: string; state: StatusDotState; working?: true }[] = [
    { name: "scout", role: "Research and reading lists", state: "running" },
    { name: "atlas", role: "Travel plans", state: "running", working: true },
    { name: "drifter", role: "Weekly review", state: "stopped" },
    { name: "brittle", role: "Home automation", state: "failed" },
  ];
  const PLACES = [
    "Home",
    "Inbox",
    ...AGENTS.map((agent) => agent.name),
    "Workbench",
    "Shared files",
  ];

  let createOpen = $state(false);
  let newName = $state("");
  let newPurpose = $state("");
  let instructionsOpen = $state(false);
  let discardOpen = $state(false);
  let instructions = $state(
    "Keep track of my reading list.\n\nWhen I finish a book, ask what I thought of it and file a short note under reading/. Suggest the next book from the list, and say why.\n\nOn Sunday evenings, summarize the week: what I read, what I abandoned, and what is overdue at the library.",
  );
  let sheetOpen = $state(false);
  let chosenAgent = $state("scout");
  let drawerOpen = $state(false);
  let answer = $state("");

  async function askDelete(): Promise<void> {
    const confirmed = await confirmations.ask({
      title: "Delete brittle?",
      message:
        "Its workspace, conversation and settings are removed. The notice that follows offers Undo.",
      confirmLabel: "Delete brittle",
      tone: "danger",
    });
    answer = confirmed ? "Deleted brittle." : "Kept brittle.";
  }

  async function askLeave(): Promise<void> {
    const leave = await confirmLeave([
      "Unsaved changes to SOUL.md",
      "Staged model settings for atlas",
    ]);
    answer = leave ? "Left without saving." : "Kept editing.";
  }
</script>

<div class="gallery" data-ui>
  <main class="gallery-page">
    <header class="gallery-head">
      <h1>Primitives</h1>
      <p>Every control in every state. Development and mock builds only.</p>
    </header>

    <section aria-labelledby="g-buttons">
      <h2 id="g-buttons">Buttons</h2>
      <div class="gallery-surface">
        {#each VARIANTS as { variant, label } (variant)}
          <div class="gallery-row">
            <span class="gallery-caption">{variant}</span>
            <Button {variant}>{label}</Button>
            <Button {variant} icon="plus">{label}</Button>
            <Button {variant} size="sm">{label}</Button>
            <Button {variant} loading>{label}</Button>
            <Button {variant} disabled>{label}</Button>
          </div>
        {/each}
      </div>
    </section>

    <section aria-labelledby="g-icon-buttons">
      <h2 id="g-icon-buttons">Icon buttons</h2>
      <div class="gallery-surface">
        <div class="gallery-row">
          <span class="gallery-caption">variants</span>
          <IconButton icon="settings" label="Settings" />
          <IconButton icon="more" label="More actions" variant="secondary" />
          <IconButton icon="send" label="Send" variant="primary" />
          <IconButton icon="close" label="Remove" variant="danger" />
        </div>
        <div class="gallery-row">
          <span class="gallery-caption">states</span>
          <IconButton
            icon="settings"
            label="Settings"
            pressed={settingsOpen}
            onclick={() => {
              settingsOpen = !settingsOpen;
            }}
          />
          <IconButton icon="settings" label="Settings, open" pressed />
          <IconButton icon="reload" label="Reload" size="sm" />
          <IconButton icon="reload" label="Reloading" loading />
          <IconButton icon="send" label="Send" variant="primary" disabled />
        </div>
      </div>
    </section>

    <section aria-labelledby="g-fields">
      <h2 id="g-fields">Text, number and select</h2>
      <div class="gallery-grid">
        <div class="gallery-surface gallery-form">
          <TextField
            label="Name"
            bind:value={name}
            hint="Lowercase letters, numbers and hyphens."
          />
          <TextField
            label="Name, invalid"
            bind:value={badName}
            hint="Lowercase letters, numbers and hyphens."
            error="Use lowercase letters, numbers and hyphens only."
          />
          <TextField
            label="What should it help with?"
            bind:value={notes}
            multiline
            placeholder="Keep track of my reading list"
          />
          <TextField label="Workspace" value="~/.residuum/atlas/workspace" code readonly />
          <TextField label="Disabled" value="Set by the hub" disabled />
        </div>
        <div class="gallery-surface gallery-form">
          <NumberField
            label="Earlier messages to read"
            bind:value={context}
            unit="messages"
            min={0}
            max={100}
            hint="When someone mentions atlas, it reads this many earlier messages first."
          />
          <NumberField
            label="Listener port"
            bind:value={port}
            error="Ports below 1024 need administrator rights. Pick 1024 or higher."
          />
          <SelectField label="Provider" bind:value={provider} options={PROVIDERS} />
          <SelectField
            label="Provider, none chosen"
            bind:value={unsetProvider}
            options={PROVIDERS}
            placeholder="Choose a provider"
          />
          <SelectField
            label="Model"
            bind:value={model}
            options={[{ value: "claude-9", label: "claude-9 (not offered)" }]}
            error="Anthropic doesn't offer this model. Choose another."
          />
          <SelectField label="Model, loading" value="" options={[]} loading />
          <SelectField label="Disabled" value="anthropic" options={PROVIDERS} disabled />
        </div>
      </div>
    </section>

    <section aria-labelledby="g-choices">
      <h2 id="g-choices">Toggles and segmented controls</h2>
      <div class="gallery-grid">
        <div class="gallery-surface gallery-form">
          <Toggle
            label="Let others talk to this agent"
            bind:checked={others}
            hint="Off: only you can talk to atlas. On: anyone who can message the bot can."
          />
          <Toggle label="Regular checks" bind:checked={pulses} />
          <Toggle label="Disabled" checked disabled />
          <Toggle label="Saving" checked loading />
          <Toggle label="Start automatically" layout="inline" bind:checked={autostart} />
        </div>
        <div class="gallery-surface gallery-form">
          <SegmentedControl
            label="Thinking"
            bind:value={thinking}
            options={THINKING}
            hint="More thinking helps with multi-step work. Replies take longer and cost more."
          />
          <SegmentedControl
            label="Thinking, invalid"
            bind:value={invalidThinking}
            options={THINKING}
            error="This model doesn't support high thinking."
          />
          <SegmentedControl
            label="One option off"
            value="low"
            options={[
              { value: "off", label: "Off" },
              { value: "low", label: "Low" },
              { value: "high", label: "High", disabled: true },
            ]}
          />
          <SegmentedControl label="Disabled" value="off" options={THINKING} disabled />
        </div>
      </div>
    </section>

    <section aria-labelledby="g-secrets">
      <h2 id="g-secrets">Secret fields</h2>
      <div class="gallery-surface gallery-form">
        <SecretField label="API key" source={{ kind: "stored" }} bind:value={storedKey} />
        <SecretField
          label="Bot token"
          source={{ kind: "env", variable: "DISCORD_TOKEN" }}
          bind:value={envKey}
        />
        <SecretField
          label="OpenAI API key"
          source={{ kind: "none" }}
          bind:value={newKey}
          placeholder="Paste the key from your OpenAI account"
        />
        <SecretField
          label="Replacing a key"
          source={{ kind: "stored" }}
          bind:value={replacingKey}
          editing
          hint="The new key is stored when you save changes."
        />
        <SecretField
          label="Search key"
          source={{ kind: "none" }}
          value=""
          error="Paste a key first. Brave Search shows it under API keys."
        />
      </div>
    </section>

    <section aria-labelledby="g-badges">
      <h2 id="g-badges">Badges and status</h2>
      <div class="gallery-surface">
        <div class="gallery-row">
          <span class="gallery-caption">counts</span>
          <Badge count={3} />
          <Badge count={3} solid label="unread" />
          <Badge count={42} tone="neutral" />
          <Badge count={120} solid label="unread" />
          <Badge count={2} tone="danger" label="problems" />
        </div>
        <div class="gallery-row">
          <span class="gallery-caption">labels</span>
          <Badge>paused</Badge>
          <Badge tone="accent" dot>running</Badge>
          <Badge tone="positive" dot>Connected</Badge>
          <Badge tone="danger" dot>overlap</Badge>
        </div>
        <div class="gallery-row">
          <span class="gallery-caption">agent state</span>
          {#each STATES as { state, working, word } (word)}
            <span class="gallery-state">
              <StatusDot {state} {working} />
              {word}
            </span>
          {/each}
          <StatusDot state="failed" label="brittle, failed" />
        </div>
      </div>
    </section>

    <section aria-labelledby="g-structure">
      <h2 id="g-structure">Disclosure and tabs</h2>
      <div class="gallery-grid">
        <div class="gallery-surface gallery-form">
          <Disclosure summary="Use different models for specific jobs">
            <p class="gallery-note">Summarizing older messages, condensing memories and more.</p>
          </Disclosure>
          <Disclosure summary="More options" bind:open={jobsOpen}>
            <p class="gallery-note">
              Open from the start, and remembers what's inside when closed.
            </p>
          </Disclosure>
          <Disclosure summary="Details" tone="quiet">
            <pre class="gallery-pre">error: provider "anthropic" rejected model "claude-9"</pre>
          </Disclosure>
        </div>
        <div class="gallery-surface">
          <Tabs
            label="Inbox"
            bind:selected={inboxTab}
            tabs={[
              { value: "inbox", label: "Inbox", count: 3 },
              { value: "archived", label: "Archived" },
              { value: "later", label: "Later", disabled: true },
            ]}
          >
            {#snippet children(tab)}
              {#if tab === "inbox"}
                <p class="gallery-note">Three things need you.</p>
              {:else}
                <EmptyState>Nothing archived yet.</EmptyState>
              {/if}
            {/snippet}
          </Tabs>
        </div>
      </div>
    </section>

    <section aria-labelledby="g-empty">
      <h2 id="g-empty">Empty and loading</h2>
      <div class="gallery-grid">
        <div class="gallery-surface">
          <EmptyState>
            Nothing running. Work atlas starts on its own shows up here.
            {#snippet actions()}
              <Button variant="quiet" size="sm">Start a session</Button>
            {/snippet}
          </EmptyState>
          <EmptyState variant="block" icon="inbox" title="You're all caught up">
            Agents put things here that need you.
            {#snippet actions()}
              <Button variant="secondary" icon="chat">Open a chat</Button>
            {/snippet}
          </EmptyState>
        </div>
        <div class="gallery-surface gallery-form">
          <Skeleton lines={3} label="Loading sessions" />
          <Skeleton shape="block" />
          <div class="gallery-row">
            <Skeleton shape="circle" />
            <Skeleton width="40%" />
          </div>
        </div>
      </div>
    </section>

    <section aria-labelledby="g-banners">
      <h2 id="g-banners">Banners</h2>
      <div class="gallery-surface gallery-form">
        <Banner>Residuum Cloud hasn't reported a workbench address yet.</Banner>
        <Banner icon="check">
          Saved. atlas is still stopped. Restart it to use the new model.
          {#snippet actions()}
            <Button variant="primary" size="sm" icon="reload">Restart atlas</Button>
          {/snippet}
        </Banner>
        <Banner busy>Starting atlas…</Banner>
        <Banner tone="warn" title="Reconnecting.">3 messages will send once back online.</Banner>
        <Banner tone="error" title="Couldn't load sessions.">
          Check that Residuum is running, then try again.
          {#snippet actions()}
            <Button variant="secondary" size="sm">Try again</Button>
          {/snippet}
        </Banner>
        {#if showDismissible}
          <Banner
            ondismiss={() => {
              showDismissible = false;
            }}
          >
            An update is ready. It installs the next time Residuum restarts.
          </Banner>
        {/if}
        <Banner tone="error" edge>
          Can't reach Residuum.
          {#snippet actions()}
            <Button variant="secondary" size="sm">Retry</Button>
          {/snippet}
        </Banner>
      </div>
    </section>

    <section aria-labelledby="g-overlays">
      <h2 id="g-overlays">Dialogs, sheets and drawers</h2>
      <div class="gallery-surface">
        <div class="gallery-row">
          <span class="gallery-caption">dialogs</span>
          <Button icon="plus" onclick={() => (createOpen = true)}>Create agent</Button>
          <Button icon="edit" onclick={() => (instructionsOpen = true)}>Edit instructions</Button>
        </div>
        <div class="gallery-row">
          <span class="gallery-caption">phone layers</span>
          <Button onclick={() => (sheetOpen = true)}>Switch agent</Button>
          <Button icon="menu" onclick={() => (drawerOpen = true)}>Agents and places</Button>
        </div>
        <div class="gallery-row">
          <span class="gallery-caption">confirm</span>
          <Button variant="danger" onclick={askDelete}>Delete brittle</Button>
          <Button onclick={askLeave}>Leave with unsaved changes</Button>
          <span class="gallery-note" role="status">{answer}</span>
        </div>
      </div>
    </section>

    <section aria-labelledby="g-kbd">
      <h2 id="g-kbd">Keys</h2>
      <div class="gallery-surface">
        <div class="gallery-row">
          <span class="gallery-caption">this device</span>
          <Kbd keys={["Mod", "K"]} />
          <Kbd keys={["?"]} />
          <Kbd keys={["Esc"]} />
        </div>
        <div class="gallery-row">
          <span class="gallery-caption">apple / other</span>
          <Kbd keys={["Mod", "Shift", "P"]} platform="apple" />
          <Kbd keys={["Mod", "Shift", "P"]} platform="other" />
        </div>
      </div>
    </section>
  </main>
</div>

<Dialog
  bind:open={createOpen}
  title="Create an agent"
  description="Give it a name and say what it should help with. It asks you about the rest."
>
  <form
    id="g-create"
    class="gallery-form-dialog"
    onsubmit={(event) => {
      event.preventDefault();
      createOpen = false;
    }}
  >
    <TextField
      label="Name"
      bind:value={newName}
      placeholder="research-buddy"
      hint="Lowercase letters, numbers and hyphens. You can't change it later."
    />
    <TextField
      label="What should it help with?"
      bind:value={newPurpose}
      multiline
      rows={3}
      placeholder="Keep track of my reading list"
    />
  </form>
  {#snippet actions()}
    <Button variant="quiet" onclick={() => (createOpen = false)}>Cancel</Button>
    <Button variant="primary" type="submit" form="g-create">Create agent</Button>
  {/snippet}
</Dialog>

<Dialog bind:open={instructionsOpen} title="scout's instructions" size="lg" fullscreenOnPhone>
  <TextField label="SOUL.md" bind:value={instructions} multiline rows={12} />
  {#snippet actions()}
    <Button variant="danger" onclick={() => (discardOpen = true)}>Discard changes</Button>
    <Button variant="primary" onclick={() => (instructionsOpen = false)}>Save</Button>
  {/snippet}
</Dialog>

<ConfirmDialog
  bind:open={discardOpen}
  title="Discard your changes?"
  message="scout keeps the instructions it had before you started editing."
  confirmLabel="Discard changes"
  cancelLabel="Keep editing"
  tone="danger"
  onconfirm={() => (instructionsOpen = false)}
/>

<Sheet bind:open={sheetOpen} title="Switch agent">
  <div class="gallery-options" role="group" aria-label="Agents">
    {#each AGENTS as agent (agent.name)}
      <button
        type="button"
        class="gallery-option"
        aria-pressed={chosenAgent === agent.name}
        onclick={() => {
          chosenAgent = agent.name;
          sheetOpen = false;
        }}
      >
        <StatusDot state={agent.state} working={agent.working} />
        <span class="gallery-option-text">
          <span class="gallery-option-name">{agent.name}</span>
          <span class="gallery-option-role">{agent.role}</span>
        </span>
      </button>
    {/each}
  </div>
</Sheet>

<Drawer bind:open={drawerOpen} label="Agents and places">
  <nav class="gallery-drawer" aria-label="Agents and places">
    <div class="gallery-drawer-top">
      <span class="gallery-drawer-title">Residuum</span>
      <IconButton
        icon="close"
        label="Close menu"
        data-overlay-close
        onclick={() => (drawerOpen = false)}
      />
    </div>
    {#each PLACES as place (place)}
      <button type="button" class="gallery-place" onclick={() => (drawerOpen = false)}>
        {place}
      </button>
    {/each}
  </nav>
</Drawer>

<ConfirmHost />

<style>
  .gallery {
    min-height: 100%;
  }

  .gallery-page {
    display: flex;
    flex-direction: column;
    gap: var(--space-40);
    max-width: 1040px;
    margin: 0 auto;
    padding: var(--space-32) var(--space-24) var(--space-64);
  }

  .gallery-head {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  h1 {
    font-size: var(--font-size-title);
    font-weight: var(--font-weight-semibold);
  }

  .gallery-head p,
  .gallery-note {
    color: var(--color-text-2);
  }

  h2 {
    margin-bottom: var(--space-12);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text-2);
  }

  .gallery-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 380px), 1fr));
    gap: var(--space-16);
    align-items: start;
  }

  .gallery-surface {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    min-width: 0;
    padding: var(--space-18);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);
  }

  .gallery-form {
    gap: var(--space-20);
  }

  .gallery-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-12);
  }

  .gallery-caption {
    flex: none;
    width: 96px;
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
  }

  .gallery-state {
    display: inline-flex;
    align-items: center;
    gap: var(--space-4);
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .gallery-pre {
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .gallery-form-dialog {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .gallery-options,
  .gallery-drawer {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .gallery-option {
    display: flex;
    align-items: center;
    gap: var(--space-12);
    min-height: 56px;
    padding: var(--space-6) var(--space-8);
    border-radius: var(--corner-md);
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover,
    &[aria-pressed="true"] {
      background: var(--color-stone-4);
    }
  }

  .gallery-option-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .gallery-option-name {
    font-weight: var(--font-weight-semibold);
  }

  .gallery-option-role {
    font-size: var(--font-size-xs);
    color: var(--color-text-2);
  }

  .gallery-drawer {
    padding: var(--space-8) var(--space-10) var(--space-24);
  }

  .gallery-drawer-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 0 var(--space-8) var(--space-8);
  }

  .gallery-drawer-title {
    font-size: var(--font-size-heading);
    font-weight: var(--font-weight-semibold);
  }

  .gallery-place {
    min-height: 40px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-3);
      color: var(--color-text);
    }
  }

  @media (max-width: 760px) {
    .gallery-page {
      padding: var(--space-24) var(--space-16) var(--space-48);
    }

    .gallery-caption {
      width: 100%;
    }
  }
</style>
