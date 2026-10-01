<script lang="ts">
  import type { Snippet } from "svelte";
  import { configFileName } from "../../lib/config-coordinator";
  import { router } from "../../lib/router.svelte";
  import { sectionFiles, type SectionId } from "../../lib/settings-sections";
  import type { Diagnostic } from "../../lib/types";
  import { Banner, Button } from "../../lib/ui";
  import type { SettingsScope } from "./sections";

  // The top of every settings section: its title, a line on what it holds,
  // and the problems the last save found that none of its fields shows. The
  // section's own content follows. While Raw config holds unsaved edits to a
  // file this section edits, the content is read-only.

  interface Props {
    scope: SettingsScope;
    section: SectionId;
    title: string;
    lede: string;
    /** Problems to show here instead of the scope's section problems, for content that can't show a field's own. */
    problems?: readonly Diagnostic[];
    children: Snippet;
  }

  let { scope, section, title, lede, problems, children }: Props = $props();

  const shown = $derived(problems ?? scope.sectionDiagnostics(section));
  const rawLocked = $derived(
    sectionFiles(scope.kind, section)
      .map((name) => scope.file(name))
      .filter((file) => file?.lockedBy === "raw")
      .map((file) => (file === undefined ? "" : configFileName(file.file))),
  );
</script>

<header class="section-head">
  <h2 class="section-title">{title}</h2>
  <p class="section-lede">{lede}</p>
</header>
{#if rawLocked.length > 0}
  <div class="section-problem">
    <Banner>
      You have unsaved edits to {rawLocked.join(" and ")} in Raw config. Save or discard them there to
      change these settings.
      {#snippet actions()}
        <Button size="sm" onclick={() => void router.switchSettingsSection("raw")}>
          Open Raw config
        </Button>
      {/snippet}
    </Banner>
  </div>
{/if}
{#each shown as problem, index (index)}
  <div class="section-problem">
    <Banner tone={problem.severity === "error" ? "error" : "warn"}>{problem.message}</Banner>
  </div>
{/each}
<fieldset class="section-fields" role="none" disabled={rawLocked.length > 0}>
  {@render children()}
</fieldset>

<style>
  .section-head {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    margin-bottom: var(--space-20);
  }

  .section-title {
    font-size: var(--font-size-title);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .section-lede {
    max-width: 640px;
    color: var(--color-text-2);
  }

  .section-problem {
    max-width: 640px;
    margin-bottom: var(--space-16);
  }

  /* It only disables what it holds; the content lays out as if it weren't there. */
  .section-fields {
    display: contents;
  }
</style>
