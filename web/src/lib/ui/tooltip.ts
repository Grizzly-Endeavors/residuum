/**
 * The seam between icon-only controls and whatever draws tooltips.
 *
 * A tooltip provider set in context turns a text into an attachment for the
 * control it describes. IconButton asks for its label this way, so every
 * icon-only button gets a tooltip wherever a provider is set, and none where
 * it isn't.
 */

import { getContext, setContext } from "svelte";
import type { Attachment } from "svelte/attachments";

export type TooltipProvider = (text: string) => Attachment<HTMLElement>;

const TOOLTIP_PROVIDER = Symbol("ui-tooltip-provider");

/** Makes `provider` draw the tooltips of every control below this component. */
export function provideTooltips(provider: TooltipProvider): void {
  setContext(TOOLTIP_PROVIDER, provider);
}

/** The provider set by an ancestor, if any. Call during component setup. */
export function tooltipProvider(): TooltipProvider | undefined {
  return getContext<TooltipProvider | undefined>(TOOLTIP_PROVIDER);
}
