<script lang="ts">
  import { categoryDescription } from "../lib/session-format";

  let { category }: { category: string } = $props();

  let description = $derived.by(() => {
    if (category === "scheduled" || category === "external" || category === "spawned") {
      return categoryDescription(category);
    }
    return category === "main" ? "The main agent" : category;
  });
</script>

<span class="session-badge badge-{category}" title={description}>{category}</span>
