<script lang="ts">
  let { name, args }: { name: string; args: Record<string, unknown> } = $props();

  function str(key: string): string {
    const v = args[key];
    if (v == null) return "";
    if (typeof v === "string") return v;
    if (typeof v === "number" || typeof v === "boolean") return String(v);
    return JSON.stringify(v);
  }

  function hasKey(key: string): boolean {
    return args[key] != null && args[key] !== "";
  }

  const entries = $derived(Object.entries(args).filter(([, v]) => v != null && v !== ""));

  const hasArgs = $derived(entries.length > 0);

  /** Build a metadata line from optional fields, joined with · */
  function metaLine(parts: [string, string | undefined][]): string {
    return parts
      .filter((p): p is [string, string] => p[1] != null && p[1] !== "")
      .map(([label, value]) => `${label}: ${value}`)
      .join(" · ");
  }
</script>

{#if !hasArgs}
  <!-- No arguments — tool name in header is sufficient -->
{:else if name === "exec"}
  <div class="tool-args">
    <span class="tool-args-command">$ {str("command")}</span>
    {#if hasKey("timeout_secs") && args["timeout_secs"] !== 120}
      <span class="tool-args-meta">timeout: {args["timeout_secs"]}s</span>
    {/if}
  </div>
{:else if name === "read"}
  <div class="tool-args">
    <span class="tool-args-path">{str("path")}</span>
    {#if hasKey("offset") || hasKey("limit")}
      <span class="tool-args-meta">
        {#if args["offset"] != null && args["limit"] != null}
          lines {Number(args["offset"])}–{Number(args["offset"]) + Number(args["limit"])}
        {:else if args["offset"] != null}
          from line {Number(args["offset"])}
        {:else if args["limit"] != null}
          first {Number(args["limit"])} lines
        {/if}
      </span>
    {/if}
  </div>
{:else if name === "write"}
  <div class="tool-args">
    <span class="tool-args-path">{str("path")}</span>
  </div>
{:else if name === "edit"}
  <div class="tool-args">
    <span class="tool-args-path">{str("path")}</span>
    {#if hasKey("instructions")}
      <span class="tool-args-meta">{str("instructions")}</span>
    {/if}
  </div>
{:else if name === "memory_search"}
  {@const meta = metaLine([
    ["Source", args["source"] as string | undefined],
    ["Since", args["date_from"] as string | undefined],
    ["Until", args["date_to"] as string | undefined],
    ["Project", args["project_context"] as string | undefined],
    ["Limit", args["limit"] != null ? str("limit") : undefined],
  ])}
  <div class="tool-args">
    <span class="tool-args-query">"{str("query")}"</span>
    {#if meta}
      <span class="tool-args-meta">{meta}</span>
    {/if}
  </div>
{:else if name === "ollama_web_search"}
  <div class="tool-args">
    <span class="tool-args-query">"{str("query")}"</span>
    {#if hasKey("max_results")}
      <span class="tool-args-meta">max results: {args["max_results"]}</span>
    {/if}
  </div>
{:else if name === "web_fetch"}
  <div class="tool-args">
    <span class="tool-args-url">{str("url")}</span>
  </div>
{:else if name === "send_message"}
  <div class="tool-args">
    <span class="tool-args-meta">To: {str("endpoint")}</span>
    {#if hasKey("title")}
      <span class="tool-args-label">{str("title")}</span>
    {/if}
    <span class="tool-args-quote">"{str("message")}"</span>
  </div>
{:else if name === "subagent_spawn"}
  <div class="tool-args">
    {#if hasKey("agent_name")}
      <span class="tool-args-label">Agent: {str("agent_name")}</span>
    {/if}
    {#if hasKey("model_override")}
      <span class="tool-args-meta">Model: {str("model_override")}</span>
    {/if}
    <span class="tool-args-quote">"{str("task")}"</span>
  </div>
{:else if name === "schedule_action"}
  {@const meta = metaLine([
    ["At", args["run_at"] as string | undefined],
    ["Agent", args["agent_name"] as string | undefined],
    ["Tier", args["model_tier"] as string | undefined],
  ])}
  <div class="tool-args">
    <span class="tool-args-label">{str("name")}</span>
    {#if meta}
      <span class="tool-args-meta">{meta}</span>
    {/if}
    <span class="tool-args-quote">"{str("prompt")}"</span>
  </div>
{:else if name === "inbox_list"}
  <div class="tool-args">
    {#if args["unread_only"]}
      <span class="tool-args-meta">unread only</span>
    {/if}
  </div>
{:else if name === "inbox_archive"}
  <div class="tool-args">
    {#if Array.isArray(args["ids"])}
      <span class="tool-args-meta"
        >{(args["ids"] as string[]).length} item{(args["ids"] as string[]).length === 1
          ? ""
          : "s"}</span
      >
    {/if}
  </div>
{:else if name === "project_activate" || name === "project_create" || name === "project_archive" || name === "skill_activate" || name === "skill_deactivate" || name === "switch_endpoint"}
  <div class="tool-args">
    <span class="tool-args-label">{str("name") || str("endpoint")}</span>
    {#if name === "project_create" && hasKey("description")}
      <span class="tool-args-meta">{str("description")}</span>
    {/if}
  </div>
{:else if name === "memory_get"}
  <div class="tool-args">
    <span class="tool-args-label">{str("episode_id")}</span>
  </div>
{:else if name === "inbox_read" || name === "stop_agent" || name === "cancel_action"}
  <div class="tool-args">
    <span class="tool-args-label">{str("id") || str("task_id")}</span>
  </div>
{:else}
  <!-- Generic fallback: key-value display -->
  <div class="tool-args">
    <div class="tool-args-kv">
      {#each entries as [key] (key)}
        <span class="tool-args-kv-key">{key}</span>
        <span class="tool-args-kv-val">{str(key)}</span>
      {/each}
    </div>
  </div>
{/if}
