// The action registry: the one list of named things the user can do, such as
// going to a place, opening a setting, running a chat action, starting an
// agent or sending feedback. The command palette, the composer's `/` menu and
// the rail's help menu all draw from it.
//
// A source is a function that builds its actions from current state. The
// registry calls every source inside one derivation, so the list follows the
// agents, the bound agent and its sessions without anyone refreshing it.

import { tick } from "svelte";
import type { IconName } from "./icons";
import type { StatusDotState } from "./ui/types";

/** The palette's heading for the help actions, which the rail's help menu also lists. */
export const HELP_GROUP = "Help";

/** An agent's state mark, drawn in place of an icon. */
export interface ActionDot {
  readonly dot: StatusDotState;
  readonly working: boolean;
}

export interface AppAction {
  /** Unique in the registry, such as `go:home` or `chat:observe`. */
  readonly id: string;
  /** The heading it is listed under in the palette. */
  readonly group: string;
  /** What it does, in plain words: "Summarize older messages now". */
  readonly label: string;
  /** Quiet context at the end of its row: the agent it acts on, or a state. */
  readonly hint?: string;
  readonly icon: IconName | ActionDot;
  /** More words it is found by, such as an agent's role. */
  readonly terms?: readonly string[];
  /** Its name in the composer, typed after `/`. Only chat actions have one. */
  readonly command?: string;
  /** It acts on text: what follows `/name` in the composer, or else it asks for some. */
  readonly takesText?: boolean;
  /** Why it can't run now, in plain words. Absent when it can. */
  readonly disabled?: string;
  /** Listed only once something is typed, to keep the unsearched list short. */
  readonly searchOnly?: boolean;
  readonly run: (text?: string) => void;
}

export type ActionSource = () => readonly AppAction[];

export class ActionRegistry {
  private sources = $state.raw<readonly { key: string; build: ActionSource }[]>([]);
  private runListeners: (() => void)[] = [];

  /** Every action, in the order the sources registered. */
  readonly all: readonly AppAction[] = $derived(this.sources.flatMap((source) => source.build()));

  /** Add a source under `key`, replacing one already there. Returns a function that removes it. */
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

  /** Hear every action that runs, just before it does. The shell closes its drawer here. */
  onRun(listener: () => void): () => void {
    this.runListeners.push(listener);
    return () => {
      this.runListeners = this.runListeners.filter((other) => other !== listener);
    };
  }

  /**
   * Run `action`, unless it is disabled. Listeners hear of it first, and the
   * action runs once the page has settled their changes, so a dialog it opens
   * takes focus from a page with nothing left over on top.
   */
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

/**
 * The actions `query` finds: those whose label, hint, heading, command or
 * terms hold every word of it. With nothing typed, every action that isn't
 * search-only. The registry's order is kept, so headings stay together.
 */
export function matchActions(actions: readonly AppAction[], query: string): AppAction[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return actions.filter((action) => action.searchOnly !== true);
  return actions.filter((action) => {
    const text = searchText(action);
    return words.every((word) => text.includes(word));
  });
}

/** The actions the composer's `/` reaches: those with a command. */
export function commandActions(actions: readonly AppAction[]): AppAction[] {
  return actions.filter((action) => action.command !== undefined);
}

/** A composer line that runs a chat action: the action its first word names, and the text after it. */
export interface CommandLine {
  readonly action: AppAction;
  readonly text: string;
}

/**
 * Read `line` as a command. It is one only when its first word is `/` and the
 * name of one of `actions`; anything else, such as a pasted path like
 * `/home/bear/app.log has the error`, is a message and gives null.
 */
export function readCommandLine(actions: readonly AppAction[], line: string): CommandLine | null {
  const match = /^\/(\S+)\s*([\s\S]*)$/.exec(line.trim());
  if (match === null) return null;
  const name = (match[1] ?? "").toLowerCase();
  const action = actions.find((candidate) => candidate.command === name);
  if (action === undefined) return null;
  return { action, text: (match[2] ?? "").trim() };
}

/** Consecutive actions under one heading, as the palette shows them. */
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
