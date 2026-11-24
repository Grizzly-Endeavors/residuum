import type { Component } from "svelte";
import type { FieldRef } from "../../lib/settings-fields";
import type { AgentScopeModel, AllScopeModel } from "../../lib/settings-model.svelte";
import type { AgentSectionId, AllSectionId } from "../../lib/settings-sections";
import AgentToAgent from "./AgentToAgent.svelte";
import AutoModeSection from "./AutoModeSection.svelte";
import CloudSection from "./CloudSection.svelte";
import ConnectionsSection from "./ConnectionsSection.svelte";
import DiagnosticsSection from "./DiagnosticsSection.svelte";
import GeneralSection from "./GeneralSection.svelte";
import HistorySection from "./HistorySection.svelte";
import KeysSection from "./KeysSection.svelte";
import LimitsSection from "./LimitsSection.svelte";
import ListenerSection from "./ListenerSection.svelte";
import Memory from "./Memory.svelte";
import ModelSection from "./ModelSection.svelte";
import NotificationsSection from "./NotificationsSection.svelte";
import PricingSection from "./PricingSection.svelte";
import RawConfig from "./RawConfig.svelte";
import Runtime from "./Runtime.svelte";
import Schedule from "./Schedule.svelte";
import SystemOneSection from "./SystemOneSection.svelte";
import ToolServers from "./ToolServers.svelte";
import ToolsSection from "./ToolsSection.svelte";
import UpdatesSection from "./UpdatesSection.svelte";

export type SettingsScope = AgentScopeModel | AllScopeModel;

export interface AgentSectionProps {
  scope: AgentScopeModel;
  section: AgentSectionId;
}

export interface AllSectionProps {
  scope: AllScopeModel;
  section: AllSectionId;
}

export const AGENT_SECTION_VIEWS: Record<AgentSectionId, Component<AgentSectionProps>> = {
  model: ModelSection,
  connections: ConnectionsSection,
  tools: ToolsSection,
  auto_mode: AutoModeSection,
  servers: ToolServers,
  a2a: AgentToAgent,
  memory: Memory,
  schedule: Schedule,
  runtime: Runtime,
  raw: RawConfig,
  history: HistorySection,
};

export const ALL_SECTION_VIEWS: Record<AllSectionId, Component<AllSectionProps>> = {
  general: GeneralSection,
  notifications: NotificationsSection,
  cloud: CloudSection,
  keys: KeysSection,
  system_one: SystemOneSection,
  updates: UpdatesSection,
  limits: LimitsSection,
  pricing: PricingSection,
  listener: ListenerSection,
  diagnostics: DiagnosticsSection,
  raw: RawConfig,
  history: HistorySection,
};

export function fieldError(scope: SettingsScope, ref: FieldRef): string | undefined {
  const found = scope.fieldDiagnostics(ref);
  return found.length === 0 ? undefined : found.map((problem) => problem.message).join(" ");
}

export function configFieldError(
  scope: SettingsScope,
  field: Extract<FieldRef, { kind: "config" }>["field"],
): string | undefined {
  return fieldError(scope, { kind: "config", field });
}

export function scopeName(scope: SettingsScope): string {
  return scope.agent === null ? "the install-wide settings" : `${scope.agent}'s settings`;
}
