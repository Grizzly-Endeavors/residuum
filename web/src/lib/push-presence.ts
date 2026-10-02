// Telling the hub when this window is in front of the user, so it holds back
// pushes the user is already looking at. The hub ends a report
// itself when the socket closes, so nothing is sent while the socket is down,
// and a new connection hears the report again.

import type { HubClientMessage } from "./hub-types";

/** How often an active window repeats itself; the hub keeps a report for 60 seconds. */
export const PRESENCE_REPEAT_MS = 30_000;

export type PresenceMessage = Extract<HubClientMessage, { type: "presence" }>;

/** The hub socket, as presence uses it. */
export interface PresenceSocket {
  connected: () => boolean;
  send: (message: PresenceMessage) => void;
  /** Call `listener` on each new connection. Returns a function that stops. */
  onConnect: (listener: () => void) => () => void;
}

/** The window, as presence uses it. */
export interface PresencePage {
  visible: () => boolean;
  focused: () => boolean;
  /** Call `listener` when visibility or focus changes. Returns a function that stops. */
  onChange: (listener: () => void) => () => void;
}

export class PresenceReporter {
  #device: string | null = null;
  /** The device the hub was last told is active, or null. */
  #reported: string | null = null;
  #timer: ReturnType<typeof setInterval> | undefined;

  constructor(
    private readonly socket: PresenceSocket,
    private readonly page: PresencePage,
  ) {}

  /** Follow the page and the socket. Returns a function that stops and says the window is gone. */
  start(): () => void {
    const stopPage = this.page.onChange(() => {
      this.#update();
    });
    const stopSocket = this.socket.onConnect(() => {
      // The new connection has no report yet.
      this.#reported = null;
      this.#update();
    });
    this.#update();
    return () => {
      stopPage();
      stopSocket();
      this.setDevice(null);
    };
  }

  /** The device with push on here, or null when push is off. */
  setDevice(device: string | null): void {
    this.#device = device;
    this.#update();
  }

  #update(): void {
    const active =
      this.#device !== null && this.page.visible() && this.page.focused() ? this.#device : null;
    if (active === this.#reported) return;
    if (this.#reported !== null) this.#send(this.#reported, false);
    clearInterval(this.#timer);
    this.#timer = undefined;
    this.#reported = active;
    if (active === null) return;
    this.#send(active, true);
    this.#timer = setInterval(() => {
      this.#send(active, true);
    }, PRESENCE_REPEAT_MS);
  }

  #send(device: string, active: boolean): void {
    // A closed socket already ended the report, and queuing would replay a stale one.
    if (this.socket.connected()) this.socket.send({ type: "presence", device_id: device, active });
  }
}

/** The browser window: visible from its visibility state, focused from its focus and blur events. */
export function browserPresencePage(): PresencePage {
  let focused = document.hasFocus();
  return {
    visible: () => document.visibilityState === "visible",
    focused: () => focused,
    onChange: (listener) => {
      const onFocus = (event: FocusEvent): void => {
        if (event.target !== window) return;
        focused = event.type === "focus";
        listener();
      };
      document.addEventListener("visibilitychange", listener);
      window.addEventListener("focus", onFocus);
      window.addEventListener("blur", onFocus);
      return () => {
        document.removeEventListener("visibilitychange", listener);
        window.removeEventListener("focus", onFocus);
        window.removeEventListener("blur", onFocus);
      };
    },
  };
}
