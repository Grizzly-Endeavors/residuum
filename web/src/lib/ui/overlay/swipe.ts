import type { Attachment } from "svelte/attachments";

export type SwipeDirection = "down" | "left";

const CLOSE_SHARE = 0.3;
const CLOSE_MAX_PX = 120;
const FLICK_PX_PER_MS = 0.5;
const SLOP_PX = 8;

interface Gesture {
  x: number;
  y: number;
  startedAt: number;
  axis: "x" | "y" | null;
  travel: number;
  follow: boolean;
  ignored: boolean;
}

function reducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

function scrolledDown(target: EventTarget | null, layer: HTMLElement): boolean {
  for (
    let node = target instanceof Element ? target : null;
    node !== null;
    node = node.parentElement
  ) {
    if (node.scrollTop > 0) return true;
    if (node === layer) break;
  }
  return false;
}

export function swipeToDismiss(
  direction: SwipeDirection,
  onDismiss: () => void,
): Attachment<HTMLElement> {
  return (element) => {
    let gesture: Gesture | null = null;

    const settle = (): void => {
      element.removeAttribute("data-swiping");
      element.style.transform = "";
    };

    const onStart = (event: TouchEvent): void => {
      const touch = event.touches[0];
      gesture =
        event.touches.length === 1 && touch !== undefined
          ? {
              x: touch.clientX,
              y: touch.clientY,
              startedAt: event.timeStamp,
              axis: null,
              travel: 0,
              follow: !reducedMotion(),
              ignored: direction === "down" && scrolledDown(event.target, element),
            }
          : null;
    };

    const onMove = (event: TouchEvent): void => {
      const touch = event.touches[0];
      if (gesture === null || gesture.ignored || touch === undefined) return;
      const dx = touch.clientX - gesture.x;
      const dy = touch.clientY - gesture.y;
      if (gesture.axis === null) {
        if (Math.abs(dx) < SLOP_PX && Math.abs(dy) < SLOP_PX) return;
        gesture.axis = Math.abs(dx) > Math.abs(dy) ? "x" : "y";
        gesture.ignored = gesture.axis !== (direction === "down" ? "y" : "x");
        if (gesture.ignored) return;
      }
      gesture.travel = Math.max(0, direction === "down" ? dy : -dx);
      event.preventDefault();
      if (!gesture.follow) return;
      element.setAttribute("data-swiping", "");
      element.style.transform =
        direction === "down"
          ? `translateY(${String(gesture.travel)}px)`
          : `translateX(${String(-gesture.travel)}px)`;
    };

    const onEnd = (event: TouchEvent): void => {
      if (gesture === null) return;
      const { travel, startedAt, ignored } = gesture;
      gesture = null;
      settle();
      if (ignored) return;
      const size = direction === "down" ? element.offsetHeight : element.offsetWidth;
      const speed = travel / Math.max(1, event.timeStamp - startedAt);
      const far = travel > Math.min(CLOSE_MAX_PX, size * CLOSE_SHARE);
      if (far || (travel > SLOP_PX && speed > FLICK_PX_PER_MS)) onDismiss();
    };

    const onCancel = (): void => {
      gesture = null;
      settle();
    };

    element.addEventListener("touchstart", onStart, { passive: true });
    element.addEventListener("touchmove", onMove, { passive: false });
    element.addEventListener("touchend", onEnd);
    element.addEventListener("touchcancel", onCancel);
    return () => {
      element.removeEventListener("touchstart", onStart);
      element.removeEventListener("touchmove", onMove);
      element.removeEventListener("touchend", onEnd);
      element.removeEventListener("touchcancel", onCancel);
    };
  };
}
