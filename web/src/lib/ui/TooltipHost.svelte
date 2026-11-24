<script module lang="ts">
  import type { Attachment } from "svelte/attachments";
  import type { TooltipProvider } from "./tooltip";

  const HOVER_DELAY_MS = 500;
  const SKIP_DELAY_MS = 300;

  let shown = $state.raw<{ anchor: HTMLElement; text: string } | null>(null);
  let pending: ReturnType<typeof setTimeout> | undefined;
  let hiddenAt = Number.NEGATIVE_INFINITY;
  let fromKeyboard = false;

  function show(anchor: HTMLElement, text: string): void {
    clearTimeout(pending);
    shown = { anchor, text };
  }

  function hide(anchor?: HTMLElement): void {
    clearTimeout(pending);
    if (shown === null || (anchor !== undefined && shown.anchor !== anchor)) return;
    shown = null;
    hiddenAt = Date.now();
  }

  export const tooltip: TooltipProvider =
    (text: string): Attachment<HTMLElement> =>
    (element) => {
      let pressed = false;
      const enter = (event: PointerEvent): void => {
        if (event.pointerType === "touch" || pressed) return;
        clearTimeout(pending);
        if (shown !== null || Date.now() - hiddenAt < SKIP_DELAY_MS) show(element, text);
        else pending = setTimeout(() => show(element, text), HOVER_DELAY_MS);
      };
      const leave = (): void => {
        pressed = false;
        hide(element);
      };
      const press = (): void => {
        pressed = true;
        hide(element);
      };
      const focus = (): void => {
        if (fromKeyboard) show(element, text);
      };
      const blur = (): void => hide(element);
      const listeners = [
        ["pointerenter", enter],
        ["pointerleave", leave],
        ["pointerdown", press],
        ["focus", focus],
        ["blur", blur],
      ] as const;
      for (const [type, listener] of listeners) {
        element.addEventListener(type, listener as EventListener);
      }
      return () => {
        for (const [type, listener] of listeners) {
          element.removeEventListener(type, listener as EventListener);
        }
        hide(element);
      };
    };
</script>

<script lang="ts">
  import { flushSync } from "svelte";
  import FloatingLayer from "./FloatingLayer.svelte";

  const uid = $props.id();
  const tooltipId = `${uid}-tooltip`;

  $effect(() => {
    const key = (): void => {
      fromKeyboard = true;
    };
    const pointer = (): void => {
      fromKeyboard = false;
    };
    window.addEventListener("keydown", key, true);
    window.addEventListener("pointerdown", pointer, true);
    return () => {
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("pointerdown", pointer, true);
    };
  });

  $effect(() => {
    if (shown === null) return;
    const press = (): void => {
      hide();
      flushSync();
    };
    window.addEventListener("pointerdown", press, true);
    return () => window.removeEventListener("pointerdown", press, true);
  });

  $effect(() => {
    const current = shown;
    if (current === null || current.text === current.anchor.getAttribute("aria-label")) return;
    const before = current.anchor.getAttribute("aria-describedby");
    current.anchor.setAttribute("aria-describedby", before ? `${before} ${tooltipId}` : tooltipId);
    return () => {
      if (before === null) current.anchor.removeAttribute("aria-describedby");
      else current.anchor.setAttribute("aria-describedby", before);
    };
  });
</script>

{#if shown !== null}
  {#key shown.anchor}
    <FloatingLayer
      open
      anchor={shown.anchor}
      shape="tooltip"
      side="top"
      align="center"
      initialFocus={false}
      id={tooltipId}
      role="tooltip"
      onclose={() => hide()}
    >
      {shown.text}
    </FloatingLayer>
  {/key}
{/if}
