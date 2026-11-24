import type { HubClientMessage } from "./hub-types";

export const PRESENCE_REPEAT_MS = 30_000;

export type PresenceMessage = Extract<HubClientMessage, { type: "presence" }>;

export interface PresenceSocket {
  connected: () => boolean;
  send: (message: PresenceMessage) => void;
  onConnect: (listener: () => void) => () => void;
}

export interface PresencePage {
  visible: () => boolean;
  focused: () => boolean;
  onChange: (listener: () => void) => () => void;
}

export class PresenceReporter {
  #device: string | null = null;
  #reported: string | null = null;
  #timer: ReturnType<typeof setInterval> | undefined;

  constructor(
    private readonly socket: PresenceSocket,
    private readonly page: PresencePage,
  ) {}

  start(): () => void {
    const stopPage = this.page.onChange(() => {
      this.#update();
    });
    const stopSocket = this.socket.onConnect(() => {
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
    if (this.socket.connected()) this.socket.send({ type: "presence", device_id: device, active });
  }
}

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
