/** Types shared by the primitive controls and their callers. */

import type { Attachment } from "svelte/attachments";
import type { AgentDisplayState } from "../agent-display-state";

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

/** A labeled group of choices, rendered as an `optgroup`. */
export interface ChoiceGroup<T extends string = string> {
  readonly label: string;
  readonly options: readonly Choice<T>[];
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

/** An agent's state as a status dot draws it. */
export type StatusDotState = AgentDisplayState;

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

/** Where a modal layer sits: hung below the top edge, hung higher for a search, a bottom sheet, or a left drawer. */
export type ModalFrame = "center" | "top" | "bottom" | "left";

/** Dialog widths: 400, 480 and 640px. */
export type DialogSize = "sm" | "md" | "lg";

/** A confirm dialog's go-ahead button: primary, or danger when it loses or removes something. */
export type ConfirmTone = "default" | "danger";

/** The side of its anchor a floating layer opens on, before flipping to fit. */
export type FloatSide = "top" | "bottom" | "left" | "right";

/** Where a floating layer lines up along its anchor's side, before shifting to fit. */
export type FloatAlign = "start" | "center" | "end";

/** The card a floating layer draws: a menu, a popover, or a tooltip's label. */
export type FloatShape = "menu" | "popover" | "tooltip";

/**
 * What a menu or popover hands its trigger snippet: spread it onto the button
 * that opens it, such as `<IconButton {...props} />`.
 */
export interface FloatTriggerProps {
  readonly "aria-haspopup": "menu" | "dialog";
  readonly "aria-expanded": boolean;
  readonly "aria-controls": string | undefined;
  readonly onclick: (event: MouseEvent) => void;
  readonly onkeydown?: (event: KeyboardEvent) => void;
  /** An attachment that tells the layer which element it is anchored to. */
  readonly [key: symbol]: Attachment<HTMLElement>;
}
