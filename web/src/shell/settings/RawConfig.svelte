<script lang="ts">
  import { configCoordinator, configFileName } from "../../lib/config-coordinator";
  import { userErrorMessage } from "../../lib/errors";
  import type { FieldFile } from "../../lib/settings-fields";
  import { ConflictUnanswered, type StagedFile } from "../../lib/settings-model.svelte";
  import type { SectionId } from "../../lib/settings-sections";
  import { toast } from "../../lib/toast.svelte";
  import type { Diagnostic } from "../../lib/types";
  import { Banner, Button, Tabs } from "../../lib/ui";
  import { chooseOnConflict } from "./changed-on-disk.svelte";
  import type { SettingsScope } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";

  // The Raw config section: each of the scope's files as text, written whole
  // by its own Save even when it has problems, which then show below it. A
  // file whose form holds staged changes is read-only here until they are
  // saved or discarded.

  let { scope, section }: { scope: SettingsScope; section: SectionId } = $props();

  /** Names this editor's writes, so the settings model reads the file back after them. */
  const RAW_SOURCE = Symbol("raw-config");

  let chosen = $state<FieldFile>("config");
  let saving = $state(false);
  let problems = $state.raw<Partial<Record<FieldFile, readonly Diagnostic[]>>>({});

  const file = $derived(scope.file(chosen));
  const TAB_ORDER: readonly FieldFile[] = ["config", "providers", "mcp"];
  const tabs = $derived(
    TAB_ORDER.flatMap((name) => scope.file(name) ?? []).map((staged) => ({
      value: staged.name,
      label: configFileName(staged.file),
    })),
  );

  async function save(target: StagedFile): Promise<void> {
    const draft = target.rawDraft;
    const label = configFileName(target.file);
    if (draft === null || target.raw === null) return;
    saving = true;
    try {
      const outcome = await configCoordinator.save(target.file, {
        baseline: target.raw,
        edit: { text: draft },
        choose: chooseOnConflict,
        source: RAW_SOURCE,
      });
      if (outcome.kind === "used-disk") {
        target.setRawDraft(null);
        toast.info(`Kept ${label} as it is on disk.`);
        return;
      }
      problems = { ...problems, [target.name]: outcome.result.diagnostics ?? [] };
      toast.success(`Saved ${label}.`);
    } catch (err) {
      if (err instanceof ConflictUnanswered) toast.info(`${label} wasn't saved.`);
      else toast.error(userErrorMessage(err, { action: `Couldn't save ${label}.` }));
    } finally {
      saving = false;
    }
  }
</script>

{#snippet editor(target: StagedFile)}
  {@const label = configFileName(target.file)}
  {@const locked = target.lockedBy === "form"}
  <div class="raw-file">
    {#if locked}
      <Banner>Save or discard your form changes first.</Banner>
    {/if}
    <textarea
      class="raw-text"
      aria-label="Contents of {label}"
      spellcheck="false"
      wrap="off"
      readonly={locked}
      value={target.rawDraft ?? target.raw ?? ""}
      oninput={(event) => {
        target.setRawDraft(event.currentTarget.value);
      }}
    ></textarea>
    {#each problems[target.name] ?? [] as problem, index (index)}
      <Banner tone={problem.severity === "error" ? "error" : "warn"}>{problem.message}</Banner>
    {/each}
    <div class="raw-actions">
      {#if target.rawDraft !== null && !locked}
        <Button variant="quiet" onclick={() => target.setRawDraft(null)}>Discard edits</Button>
      {/if}
      <Button
        variant="primary"
        loading={saving}
        disabled={target.rawDraft === null || locked}
        onclick={() => void save(target)}
      >
        Save {label}
      </Button>
    </div>
  </div>
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Raw config"
  lede="The files behind the other sections. A save here writes the whole file, and any problems in it show below."
>
  {#if tabs.length > 1}
    <Tabs label="Config files" {tabs} bind:selected={chosen}>
      {#if file}{@render editor(file)}{/if}
    </Tabs>
  {:else if file}
    {@render editor(file)}
  {/if}
</SettingsSection>

<style>
  .raw-file {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding-top: var(--space-12);
  }

  .raw-text {
    min-height: 360px;
    padding: var(--space-12);
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    color: var(--color-text);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-message);
    resize: vertical;

    &:focus-visible {
      outline: none;
      border-color: var(--color-vein);
      box-shadow: 0 0 0 3px var(--color-vein-faint);
    }

    &[readonly] {
      color: var(--color-text-2);
    }
  }

  .raw-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-8);
  }
</style>
