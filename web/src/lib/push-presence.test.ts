import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  PRESENCE_REPEAT_MS,
  PresenceReporter,
  type PresenceMessage,
  type PresencePage,
  type PresenceSocket,
} from "./push-presence";

/** A hub socket that keeps what was sent, and can drop and come back. */
function fakeSocket(): PresenceSocket & {
  sent: PresenceMessage[];
  open: boolean;
  reconnect: () => void;
} {
  const listeners = new Set<() => void>();
  const socket = {
    sent: [] as PresenceMessage[],
    open: true,
    connected: () => socket.open,
    send: (message: PresenceMessage) => socket.sent.push(message),
    onConnect: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    reconnect: () => {
      socket.open = true;
      for (const listener of listeners) listener();
    },
  };
  return socket;
}

/** A window that is visible and focused until a test says otherwise. */
function fakePage(): PresencePage & {
  set: (state: { visible?: boolean; focused?: boolean }) => void;
} {
  let visible = true;
  let focused = true;
  let changed = (): void => undefined;
  return {
    visible: () => visible,
    focused: () => focused,
    onChange: (listener) => {
      changed = listener;
      return () => {
        changed = () => undefined;
      };
    },
    set: (state) => {
      visible = state.visible ?? visible;
      focused = state.focused ?? focused;
      changed();
    },
  };
}

const active = (device: string): PresenceMessage => ({
  type: "presence",
  device_id: device,
  active: true,
});
const inactive = (device: string): PresenceMessage => ({
  type: "presence",
  device_id: device,
  active: false,
});

let socket: ReturnType<typeof fakeSocket>;
let page: ReturnType<typeof fakePage>;
let reporter: PresenceReporter;
let stop: () => void;

beforeEach(() => {
  vi.useFakeTimers();
  socket = fakeSocket();
  page = fakePage();
  reporter = new PresenceReporter(socket, page);
  stop = reporter.start();
});

afterEach(() => {
  stop();
  vi.useRealTimers();
});

describe("presence", () => {
  it("says nothing until this device has push on", () => {
    vi.advanceTimersByTime(PRESENCE_REPEAT_MS * 2);
    expect(socket.sent).toEqual([]);
  });

  it("reports a focused, visible window, and repeats it every 30 seconds", () => {
    reporter.setDevice("phone");
    expect(socket.sent).toEqual([active("phone")]);
    vi.advanceTimersByTime(PRESENCE_REPEAT_MS * 2);
    expect(socket.sent).toEqual([active("phone"), active("phone"), active("phone")]);
  });

  it("clears the report on blur or when hidden, and stops repeating", () => {
    reporter.setDevice("phone");
    page.set({ focused: false });
    expect(socket.sent).toEqual([active("phone"), inactive("phone")]);
    vi.advanceTimersByTime(PRESENCE_REPEAT_MS * 2);
    expect(socket.sent).toHaveLength(2);

    page.set({ focused: true });
    page.set({ visible: false });
    expect(socket.sent.slice(2)).toEqual([active("phone"), inactive("phone")]);
  });

  it("clears the report when push is turned off, or moves it to a new device", () => {
    reporter.setDevice("phone");
    reporter.setDevice("phone-2");
    reporter.setDevice(null);
    expect(socket.sent).toEqual([
      active("phone"),
      inactive("phone"),
      active("phone-2"),
      inactive("phone-2"),
    ]);
  });

  it("sends nothing while the socket is down, and reports again on the new connection", () => {
    reporter.setDevice("phone");
    socket.open = false;
    vi.advanceTimersByTime(PRESENCE_REPEAT_MS);
    page.set({ focused: false });
    page.set({ focused: true });
    expect(socket.sent).toEqual([active("phone")]);

    socket.reconnect();
    expect(socket.sent).toEqual([active("phone"), active("phone")]);
  });

  it("says the window is gone when stopped", () => {
    reporter.setDevice("phone");
    stop();
    expect(socket.sent).toEqual([active("phone"), inactive("phone")]);
    vi.advanceTimersByTime(PRESENCE_REPEAT_MS);
    expect(socket.sent).toHaveLength(2);
  });
});
