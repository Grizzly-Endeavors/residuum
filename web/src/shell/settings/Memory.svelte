<script lang="ts">
  import { router } from "../../lib/router.svelte";
  import { Button, Disclosure } from "../../lib/ui";
  import ConfigNumber from "./ConfigNumber.svelte";
  import ConfigToggle from "./ConfigToggle.svelte";
  import { fieldError, type AgentSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // The Memory section: when the agent writes conversation down
  // as memories and tidies them, what it learns and how it reviews its own
  // replies, and how it searches what it kept. All of it lives in the agent's
  // `config.toml`, so it stays editable while the agent is stopped.

  let { scope, section }: AgentSectionProps = $props();

  const agent = $derived(scope.agent);
  const reviewing = $derived(scope.config.subconscious_enabled);
  const decaying = $derived(scope.config.search_temporal_decay);

  const SEARCH_FIELDS = [
    "search_vector_weight",
    "search_text_weight",
    "search_min_score",
    "search_candidate_multiplier",
    "search_temporal_decay",
    "search_temporal_decay_half_life_days",
  ] as const;

  // A problem on a field inside the closed disclosure opens it, so it isn't hidden.
  let searchOpen = $state(false);
  const searchProblem = $derived(
    SEARCH_FIELDS.some((field) => fieldError(scope, { kind: "config", field }) !== undefined),
  );
  $effect(() => {
    if (searchProblem) searchOpen = true;
  });
</script>

{#snippet chooseModel()}
  <Button
    variant="quiet"
    size="sm"
    icon="sliders"
    onclick={() => {
      scope.requestFocus({ kind: "role", role: "subconscious" });
      void router.switchSettingsSection("model");
    }}
  >
    Choose the model that reviews replies
  </Button>
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Memory"
  lede="When {agent} writes its conversations down as memories, what it learns from them, and how it searches what it kept."
>
  <SettingsGroup
    title="Summarizing and condensing"
    lede="As a conversation grows, {agent} turns older messages into short notes, then tidies those notes once there are too many. Sizes are in tokens; a token is about three quarters of a word."
  >
    <ConfigNumber
      {scope}
      field="observer_threshold_tokens"
      label="Start summarizing at"
      unit="tokens"
      fallback={30000}
      min={0}
      hint="Once this much conversation hasn't been summarized yet, {agent} starts the wait below."
    />
    <ConfigNumber
      {scope}
      field="observer_cooldown_secs"
      label="Wait before summarizing"
      unit="seconds"
      fallback={120}
      min={0}
      hint="How long to wait after that point, in case the conversation carries on."
    />
    <ConfigNumber
      {scope}
      field="observer_force_threshold_tokens"
      label="Summarize without waiting at"
      unit="tokens"
      fallback={60000}
      min={0}
      hint="Past this much, {agent} summarizes at once instead of waiting."
    />
    <ConfigNumber
      {scope}
      field="reflector_threshold_tokens"
      label="Condense memories at"
      unit="tokens"
      fallback={40000}
      min={0}
      hint="Once its notes add up to this much, {agent} merges and trims them."
    />
  </SettingsGroup>

  <SettingsGroup
    title="Reviewing replies"
    lede="A small model reads along and nudges {agent} when it drifts from its instructions, for example when it agrees to remember something and doesn't."
    foot={chooseModel}
  >
    <ConfigToggle
      {scope}
      field="subconscious_enabled"
      label="Review replies"
      hint="Off by default, because every turn it reviews costs one more model call."
    />
    <ConfigToggle
      {scope}
      field="subconscious_mid_turn"
      label="Also review while it works"
      disabled={!reviewing}
      hint="Besides checking each finished reply, it looks in between tool calls so it can correct course sooner."
    />
    <ConfigNumber
      {scope}
      field="subconscious_every_n_iterations"
      label="Look in every"
      unit="tool calls"
      fallback={3}
      min={1}
      disabled={!reviewing || !scope.config.subconscious_mid_turn}
      hint="While it works, how often the reviewer checks."
    />
    <ConfigNumber
      {scope}
      field="subconscious_max_transcript_tokens"
      label="Conversation it reads"
      unit="tokens"
      fallback={12000}
      min={0}
      disabled={!reviewing}
      hint="The most recent part of the conversation the reviewer reads each time. Older messages are left out to fit."
    />
  </SettingsGroup>

  <SettingsGroup
    title="Learning from conversations"
    lede="When something is worth keeping, like a correction, a preference or a hard-won fix, a background helper checks it against {agent}'s memory and saves it."
  >
    <ConfigToggle
      {scope}
      field="subconscious_learning"
      label="Learn from reviewed replies"
      disabled={!reviewing}
      hint="Needs Review replies turned on. Adds an occasional background run."
    />
    <ConfigNumber
      {scope}
      field="learning_nudge_after_turns"
      label="Also look back every"
      unit="replies"
      placeholder="0"
      min={0}
      hint="Without the reviewer, {agent} can still look back over the conversation for things worth keeping. Off at 0, which is the default."
    />
    <ConfigNumber
      {scope}
      field="subconscious_learning_cooldown_minutes"
      label="At most one learning run every"
      unit="minutes"
      fallback={240}
      min={0}
      hint="The shortest gap between learning runs."
    />
  </SettingsGroup>

  <Disclosure summary="More options" bind:open={searchOpen}>
    <SettingsGroup
      title="Searching memory"
      lede="{agent} searches by meaning and by the exact words, then merges the two. These settings tune that merge."
    >
      <ConfigNumber
        {scope}
        field="search_vector_weight"
        label="Weight of meaning"
        fallback={0.7}
        min={0}
        step={0.05}
        hint="How much a match on meaning counts. Only the ratio to the weight of exact words matters."
      />
      <ConfigNumber
        {scope}
        field="search_text_weight"
        label="Weight of exact words"
        fallback={0.3}
        min={0}
        step={0.05}
        hint="How much a match on the exact words counts."
      />
      <ConfigNumber
        {scope}
        field="search_min_score"
        label="Lowest score to show"
        fallback={0.35}
        min={0}
        max={1}
        step={0.01}
        hint="Matches that score lower are left out. Scores run from 0 to 1."
      />
      <ConfigNumber
        {scope}
        field="search_candidate_multiplier"
        label="Extra matches to compare"
        unit="× the results wanted"
        fallback={4}
        min={1}
        hint="How many matches to gather before keeping the best."
      />
      <ConfigToggle
        {scope}
        field="search_temporal_decay"
        label="Prefer recent memories"
        hint="Older memories rank lower as time passes."
      />
      <ConfigNumber
        {scope}
        field="search_temporal_decay_half_life_days"
        label="Memories lose half their rank after"
        unit="days"
        fallback={30}
        min={1}
        disabled={!decaying}
        hint="Only used while Prefer recent memories is on."
      />
    </SettingsGroup>
  </Disclosure>
</SettingsSection>
