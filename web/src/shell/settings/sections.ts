// How a settings section plugs into the Settings modal. A section is a
// component that takes its scope and its id. The frame loads the scope, swaps
// sections in its content pane, and owns the save bar, so a section only
// binds its fields to the scope's forms (`scope.config`, `scope.providers`,
// `scope.models`, `scope.mcpServers`) and starts with `SettingsSection`.
//
// A section listed here is drawn by its own component; any other shows its
// legacy panels (`LegacySection.svelte`).

import type { Component } from "svelte";
import type { FieldRef } from "../../lib/settings-fields";
import type { AgentScopeModel, AllScopeModel } from "../../lib/settings-model.svelte";
import type { AgentSectionId, AllSectionId } from "../../lib/settings-sections";
import CloudSection from "./CloudSection.svelte";
import DiagnosticsSection from "./DiagnosticsSection.svelte";
import GeneralSection from "./GeneralSection.svelte";
import HistorySection from "./HistorySection.svelte";
import LimitsSection from "./LimitsSection.svelte";
import Memory from "./Memory.svelte";
import RawConfig from "./RawConfig.svelte";
import Runtime from "./Runtime.svelte";
import Schedule from "./Schedule.svelte";
import UpdatesSection from "./UpdatesSection.svelte";

export type SettingsScope = AgentScopeModel | AllScopeModel;

/** What the frame gives an agent's section. */
export interface AgentSectionProps {
  scope: AgentScopeModel;
  section: AgentSectionId;
}

/** What the frame gives an All agents section. */
export interface AllSectionProps {
  scope: AllScopeModel;
  section: AllSectionId;
}

export const AGENT_SECTION_VIEWS: Partial<Record<AgentSectionId, Component<AgentSectionProps>>> = {
  memory: Memory,
  schedule: Schedule,
  runtime: Runtime,
  raw: RawConfig,
  history: HistorySection,
};

export const ALL_SECTION_VIEWS: Partial<Record<AllSectionId, Component<AllSectionProps>>> = {
  general: GeneralSection,
  cloud: CloudSection,
  updates: UpdatesSection,
  limits: LimitsSection,
  diagnostics: DiagnosticsSection,
  raw: RawConfig,
};

/** A field's problems from the last save as one line, for a control's `error`; undefined when it has none. */
export function fieldError(scope: SettingsScope, ref: FieldRef): string | undefined {
  const found = scope.fieldDiagnostics(ref);
  return found.length === 0 ? undefined : found.map((problem) => problem.message).join(" ");
}

/** The scope's name in a sentence: "atlas's settings", or the install-wide ones. */
export function scopeName(scope: SettingsScope): string {
  return scope.agent === null ? "the install-wide settings" : `${scope.agent}'s settings`;
}
