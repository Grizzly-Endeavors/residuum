import { getContext, setContext } from "svelte";
import type { Attachment } from "svelte/attachments";

export type TooltipProvider = (text: string) => Attachment<HTMLElement>;

const TOOLTIP_PROVIDER = Symbol("ui-tooltip-provider");

export function provideTooltips(provider: TooltipProvider): void {
  setContext(TOOLTIP_PROVIDER, provider);
}

export function tooltipProvider(): TooltipProvider | undefined {
  return getContext<TooltipProvider | undefined>(TOOLTIP_PROVIDER);
}
