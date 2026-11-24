import type { Attachment } from "svelte/attachments";
import type { AgentDisplayState } from "../agent-display-state";

export type ButtonVariant = "primary" | "secondary" | "quiet" | "danger";

export type ButtonSize = "md" | "sm";

export interface Choice<T extends string = string> {
  readonly value: T;
  readonly label: string;
  readonly disabled?: boolean;
}

export interface ChoiceGroup<T extends string = string> {
  readonly label: string;
  readonly options: readonly Choice<T>[];
}

export interface TabItem<T extends string = string> extends Choice<T> {
  readonly count?: number;
}

export type SecretSource =
  | { readonly kind: "stored" }
  | { readonly kind: "env"; readonly variable: string }
  | { readonly kind: "none" };

export type AttachmentState = "ready" | "pending" | "failed";

export type StatusDotState = AgentDisplayState;

export type BadgeTone = "neutral" | "accent" | "positive" | "danger";
export type BannerTone = "info" | "warn" | "error";

export interface FieldControl {
  readonly id: string;
  readonly labelId: string;
  readonly describedBy: string | undefined;
  readonly invalid: boolean;
}

export type ModalFrame = "center" | "top" | "bottom" | "left" | "viewer";

export type DialogSize = "sm" | "md" | "lg";

export type ConfirmTone = "default" | "danger";

export type FloatSide = "top" | "bottom" | "left" | "right";

export type FloatAlign = "start" | "center" | "end";

export type FloatShape = "menu" | "popover" | "tooltip";

export interface FloatTriggerProps {
  readonly "aria-haspopup": "menu" | "dialog";
  readonly "aria-expanded": boolean;
  readonly "aria-controls": string | undefined;
  readonly onclick: (event: MouseEvent) => void;
  readonly onkeydown?: (event: KeyboardEvent) => void;
  readonly [key: symbol]: Attachment<HTMLElement>;
}
