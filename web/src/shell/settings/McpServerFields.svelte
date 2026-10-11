<script lang="ts">
  import { untrack } from "svelte";
  import {
    argsText,
    pairsText,
    parseArgs,
    parsePairs,
    skippedLinesProblem,
  } from "../../lib/mcp-form";
  import type { McpServerEntry } from "../../lib/types";
  import { TextField } from "../../lib/ui";

  // The fields of one tool server, written straight into `server`, so an edit
  // to a server in the scope's form is staged like any other. The boxes for
  // arguments, variables and headers keep the text as typed and write what
  // it reads as on each change, so a line still being typed isn't rewritten
  // under the cursor. The parent mounts a new one for a new server object.

  type ServerField = "command" | "args" | "env" | "url" | "headers";

  interface Props {
    server: McpServerEntry;
    /** A field's problems from the last save. */
    errorOf?: (field: ServerField) => string | undefined;
    /** What the form itself says is missing. */
    missing?: Partial<Record<ServerField, string>>;
  }

  let { server, errorOf = () => undefined, missing = {} }: Props = $props();

  const typed = untrack(() => ({
    args: argsText(server.args),
    env: pairsText(server.env),
    headers: pairsText(server.headers ?? {}),
  }));
  let args = $state(typed.args);
  let env = $state(typed.env);
  let headers = $state(typed.headers);
  let envSkipped = $state<number[]>([]);
  let headersSkipped = $state<number[]>([]);

  const http = $derived(server.transport === "http");
  const errorFor = (field: ServerField, own?: string): string | undefined =>
    missing[field] ?? own ?? errorOf(field);
</script>

{#if http}
  <TextField
    label="Address"
    code
    bind:value={server.url}
    placeholder="https://tools.example.com/mcp"
    autocomplete="off"
    spellcheck="false"
    error={errorFor("url")}
  />
  <TextField
    label="Headers"
    multiline
    rows={3}
    code
    spellcheck="false"
    bind:value={headers}
    oninput={(event) => {
      const read = parsePairs(event.currentTarget.value);
      server.headers = read.pairs;
      headersSkipped = read.skipped;
    }}
    hint={"One per line, as Name=value. ${NAME} reads a variable from Residuum's environment, and ${agent-key:name} a key from Saved keys."}
    error={errorFor("headers", skippedLinesProblem(headersSkipped))}
  />
{:else}
  <TextField
    label="Command"
    code
    bind:value={server.command}
    placeholder="npx"
    autocomplete="off"
    spellcheck="false"
    error={errorFor("command")}
  />
  <TextField
    label="Arguments"
    multiline
    rows={3}
    code
    spellcheck="false"
    bind:value={args}
    oninput={(event) => {
      server.args = parseArgs(event.currentTarget.value);
    }}
    hint="One per line."
    error={errorFor("args")}
  />
  <TextField
    label="Environment variables"
    multiline
    rows={3}
    code
    spellcheck="false"
    bind:value={env}
    oninput={(event) => {
      const read = parsePairs(event.currentTarget.value);
      server.env = read.pairs;
      envSkipped = read.skipped;
    }}
    hint={"One per line, as NAME=value. ${agent-key:name} reads a key from Saved keys, so it stays out of this file."}
    error={errorFor("env", skippedLinesProblem(envSkipped))}
  />
{/if}
