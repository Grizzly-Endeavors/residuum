<script lang="ts">
  import type { Snippet } from "svelte";
  import type { SectionId } from "../../lib/settings-sections";
  import type { Diagnostic } from "../../lib/types";
  import { Banner } from "../../lib/ui";
  import type { SettingsScope } from "./sections";

  // The top of every settings section: its title, a line on what it holds,
  // and the problems the last save found that none of its fields shows. The
  // section's own content follows.

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
</script>

<header class="section-head">
  <h2 class="section-title">{title}</h2>
  <p class="section-lede">{lede}</p>
</header>
{#each shown as problem, index (index)}
  <div class="section-problem">
    <Banner tone={problem.severity === "error" ? "error" : "warn"}>{problem.message}</Banner>
  </div>
{/each}
{@render children()}

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
</style>
