/** Types shared by the primitive controls and their callers. */

import type { AgentState } from "../hub-types";

/**
 * How much a button asks for attention. Primary fills with `vein-dim`,
 * secondary with stone, quiet has no fill until hovered, and danger labels in
 * `err-text`.
 */
export type ButtonVariant = "primary" | "secondary" | "quiet" | "danger";

/** 32px (`md`) or 28px (`sm`) tall at wide widths; both grow to the touch target on phones. */
export type ButtonSize = "md" | "sm";

/** One choice in a select, segmented control or tab list. */
export interface Choice<T extends string = string> {
  readonly value: T;
  readonly label: string;
  readonly disabled?: boolean;
}

/** One tab, with an optional count badge. */
export interface TabItem<T extends string = string> extends Choice<T> {
  readonly count?: number;
}

/**
 * Where a secret field's current value comes from: the encrypted store, an
 * environment variable, or nowhere yet.
 */
export type SecretSource =
  | { readonly kind: "stored" }
  | { readonly kind: "env"; readonly variable: string }
  | { readonly kind: "none" };

/** An agent's lifecycle state as a status dot draws it. `stopping` is the hub's stopping set. */
export type StatusDotState = AgentState | "stopping";

/** Badge and banner tones. Each keeps to the contrast pairs its surface allows. */
export type BadgeTone = "neutral" | "accent" | "positive" | "danger";
export type BannerTone = "info" | "warn" | "error";

/** What a field frame hands the control it wraps, so the control carries the frame's labels. */
export interface FieldControl {
  /** The control's id, which the frame's `<label for>` points at. */
  readonly id: string;
  /** The label element's id, for controls labelled with `aria-labelledby`. */
  readonly labelId: string;
  /** Hint and error ids for `aria-describedby`, or undefined when there are neither. */
  readonly describedBy: string | undefined;
  readonly invalid: boolean;
}
