import { tick } from "svelte";
import type { IconName } from "./icons";
import type { StatusDotState } from "./ui/types";

export const HELP_GROUP = "Help";

export interface ActionDot {
  readonly dot: StatusDotState;
  readonly working: boolean;
}

export interface AppAction {
  readonly id: string;
  readonly group: string;
  readonly label: string;
  readonly hint?: string;
  readonly icon: IconName | ActionDot;
  readonly terms?: readonly string[];
  readonly command?: string;
  readonly takesText?: boolean;
  readonly disabled?: string;
  readonly disabledWithText?: string;
  readonly searchOnly?: boolean;
  readonly run: (text?: string) => void;
}

export type ActionSource = () => readonly AppAction[];

export class ActionRegistry {
  private sources = $state.raw<readonly { key: string; build: ActionSource }[]>([]);
  private runListeners: (() => void)[] = [];

  readonly all: readonly AppAction[] = $derived(this.sources.flatMap((source) => source.build()));

  register(key: string, build: ActionSource): () => void {
    const at = this.sources.findIndex((source) => source.key === key);
    this.sources =
      at < 0
        ? [...this.sources, { key, build }]
        : this.sources.map((source, index) => (index === at ? { key, build } : source));
    return () => {
      this.sources = this.sources.filter((source) => source.build !== build);
    };
  }

  onRun(listener: () => void): () => void {
    this.runListeners.push(listener);
    return () => {
      this.runListeners = this.runListeners.filter((other) => other !== listener);
    };
  }

  async run(action: AppAction, text?: string): Promise<boolean> {
    if (action.disabled !== undefined) return false;
    for (const listener of this.runListeners) listener();
    await tick();
    action.run(text);
    return true;
  }
}

export const actionRegistry = new ActionRegistry();

function searchText(action: AppAction): string {
  const command = action.command === undefined ? "" : `/${action.command}`;
  return [action.label, action.hint ?? "", action.group, command, ...(action.terms ?? [])]
    .join(" ")
    .toLowerCase();
}

export function matchActions(actions: readonly AppAction[], query: string): AppAction[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return actions.filter((action) => action.searchOnly !== true);
  return actions.filter((action) => {
    const text = searchText(action);
    return words.every((word) => text.includes(word));
  });
}

export function commandActions(actions: readonly AppAction[]): AppAction[] {
  return actions.filter((action) => action.command !== undefined);
}

export interface CommandLine {
  readonly action: AppAction;
  readonly text: string;
}

export function readCommandLine(actions: readonly AppAction[], line: string): CommandLine | null {
  const match = /^\/(\S+)\s*([\s\S]*)$/.exec(line.trim());
  if (match === null) return null;
  const name = (match[1] ?? "").toLowerCase();
  const action = actions.find((candidate) => candidate.command === name);
  if (action === undefined) return null;
  return { action, text: (match[2] ?? "").trim() };
}

export interface ActionGroupRun {
  readonly heading: string;
  readonly actions: readonly { readonly action: AppAction; readonly index: number }[];
}

export function groupRuns(actions: readonly AppAction[]): ActionGroupRun[] {
  const runs: { heading: string; actions: { action: AppAction; index: number }[] }[] = [];
  actions.forEach((action, index) => {
    const last = runs.at(-1);
    if (last?.heading === action.group) last.actions.push({ action, index });
    else runs.push({ heading: action.group, actions: [{ action, index }] });
  });
  return runs;
}
