<script lang="ts">
  import { untrack } from "svelte";
  import { validateConfig, validateHubConfig, validateMcp, validateProviders } from "../../lib/api";
  import { configCoordinator, configFileName, type ConfigFile } from "../../lib/config-coordinator";
  import { userErrorMessage } from "../../lib/errors";
  import type { FieldFile } from "../../lib/settings-fields";
  import {
    ConflictUnanswered,
    diagnosticsOf,
    type StagedFile,
  } from "../../lib/settings-model.svelte";
  import type { SectionId } from "../../lib/settings-sections";
  import { toast } from "../../lib/toast.svelte";
  import type { Diagnostic } from "../../lib/types";
  import { Banner, Button, Tabs } from "../../lib/ui";
  import { chooseOnConflict } from "./changed-on-disk.svelte";
  import RawEditor from "./RawEditor.svelte";
  import type { SettingsScope } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";

  // The Raw config section (design §8): each of the scope's files as text,
  // checked for problems as it is typed, and written whole by its own Save
  // even when it has problems. A file whose form holds staged changes is
  // read-only here until they are saved or discarded; the reverse holds while
  // an edit here is unsaved (see SettingsSection).

  let { scope, section }: { scope: SettingsScope; section: SectionId } = $props();

  /** Names this editor's writes, so the settings model reads the file back after them. */
  const RAW_SOURCE = Symbol("raw-config");
  /** How long typing pauses before the text is checked, as the Files editor waits. */
  const CHECK_DELAY_MS = 500;
  const TAB_ORDER: readonly FieldFile[] = ["config", "providers", "mcp"];

  /** A check of some text: what it found, or why it couldn't be made. */
  interface Check {
    text: string;
    problems: readonly Diagnostic[];
    error: string | null;
  }

  let chosen = $state<FieldFile>("config");
  let saving = $state(false);
  let checks = $state.raw<Partial<Record<FieldFile, Check>>>({});

  const file = $derived(scope.file(chosen));
  const tabs = $derived(
    TAB_ORDER.flatMap((name) => scope.file(name) ?? []).map((staged) => ({
      value: staged.name,
      label: `${configFileName(staged.file)}${staged.rawDraft === null ? "" : " (unsaved)"}`,
    })),
  );

  const shownText = (target: StagedFile): string => target.rawDraft ?? target.raw ?? "";

  function validate(target: ConfigFile, text: string): Promise<readonly Diagnostic[]> {
    if (target.kind === "hub") return validateHubConfig(text).then(diagnosticsOf);
    if (target.name === "mcp") return validateMcp(target.agent, text);
    const check = target.name === "config" ? validateConfig : validateProviders;
    return check(target.agent, text).then(diagnosticsOf);
  }

  async function runCheck(target: StagedFile, text: string): Promise<void> {
    let found: Check;
    try {
      found = { text, problems: await validate(target.file, text), error: null };
    } catch (err) {
      const error = userErrorMessage(err, { action: "Couldn't check this file for problems." });
      found = { text, problems: [], error };
    }
    // An answer for text that has since changed is dropped; the newer text's check follows.
    if (shownText(target) === text) checks = { ...checks, [target.name]: found };
  }

  $effect(() => {
    const target = file;
    if (target?.raw == null) return;
    const text = shownText(target);
    if (untrack(() => checks[target.name]?.text) === text) return;
    const timer = setTimeout(() => void runCheck(target, text), CHECK_DELAY_MS);
    return () => {
      clearTimeout(timer);
    };
  });

  function statusOf(target: StagedFile): string {
    const check = checks[target.name];
    if (check?.text !== shownText(target)) return "Checking for problems…";
    if (check.error !== null) return check.error;
    const count = check.problems.length;
    if (count === 0) return "No problems found.";
    return `${String(count)} problem${count === 1 ? "" : "s"} found.`;
  }

  async function save(target: StagedFile): Promise<void> {
    const draft = target.rawDraft;
    const baseline = target.rawDraftBase ?? target.raw;
    const label = configFileName(target.file);
    if (draft === null || baseline === null) return;
    saving = true;
    try {
      const outcome = await configCoordinator.save(target.file, {
        baseline,
        edit: { text: draft },
        choose: chooseOnConflict,
        source: RAW_SOURCE,
      });
      if (outcome.raw !== null) target.refresh(outcome.raw);
      target.setRawDraft(null);
      if (outcome.kind === "used-disk") {
        toast.info(`Kept ${label} as it is on disk, without your edits.`);
        return;
      }
      const problems = diagnosticsOf(outcome.result);
      checks = { ...checks, [target.name]: { text: outcome.raw ?? draft, problems, error: null } };
      toast.success(
        problems.length === 0
          ? `Saved ${label}.`
          : `Saved ${label}. It has problems, listed under the editor.`,
      );
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
  {@const edited = target.rawDraft !== null}
  <div class="raw-file">
    {#if locked}
      <Banner>Save or discard your form changes first.</Banner>
    {:else if edited && target.rawDraftBase !== target.raw}
      <Banner tone="warn">
        {label} changed on disk since you started editing. Saving asks which version to keep.
      </Banner>
    {:else if target.unreadable && !edited}
      <Banner tone="warn">
        {label} can't be read as it is, so the other sections show it empty. Fix the problems below and
        save.
      </Banner>
    {/if}
    <RawEditor
      name={label}
      value={shownText(target)}
      readonly={locked}
      problems={checks[target.name]?.problems ?? []}
      status={statusOf(target)}
      oninput={(text) => {
        target.setRawDraft(text);
      }}
    />
    <div class="raw-actions">
      {#if edited && !locked}
        <Button variant="quiet" disabled={saving} onclick={() => target.setRawDraft(null)}>
          Discard edits
        </Button>
      {/if}
      <Button
        variant="primary"
        loading={saving}
        disabled={!edited || locked}
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
  lede="The files behind the other sections. A save here writes the whole file, even with problems in it, and the problems are listed under it."
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
    gap: var(--space-10);
    padding-top: var(--space-12);
  }

  .raw-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-8);
  }
</style>
