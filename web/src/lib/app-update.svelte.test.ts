import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { SKIP_WAITING_MESSAGE, WORKER_URL } from "../sw/protocol";
import {
  AppUpdate,
  type ContainerLike,
  type PageLike,
  type RegistrationLike,
  type WorkerLike,
} from "./app-update.svelte";

class FakeWorker extends EventTarget implements WorkerLike {
  state = "installing";
  readonly posted: unknown[] = [];

  postMessage(message: unknown): void {
    this.posted.push(message);
  }

  /** The worker finishes installing, as a browser reports it. */
  finishInstalling(): void {
    this.state = "installed";
    this.dispatchEvent(new Event("statechange"));
  }
}

class FakeRegistration extends EventTarget implements RegistrationLike {
  waiting: FakeWorker | null = null;
  installing: FakeWorker | null = null;
  readonly update = vi.fn((): Promise<unknown> => Promise.resolve());

  /** A rebuilt app's worker is found, installs and waits. */
  foundUpdate(): FakeWorker {
    const worker = new FakeWorker();
    this.installing = worker;
    this.dispatchEvent(new Event("updatefound"));
    this.installing = null;
    this.waiting = worker;
    worker.finishInstalling();
    return worker;
  }
}

class FakeContainer extends EventTarget implements ContainerLike {
  controller: unknown = null;
  readonly registration = new FakeRegistration();
  readonly register = vi.fn(
    (_url: string): Promise<RegistrationLike> => Promise.resolve(this.registration),
  );

  /** A worker takes control of the page, as a browser reports it. */
  takeControl(): void {
    this.controller = {};
    this.dispatchEvent(new Event("controllerchange"));
  }
}

class FakePage implements PageLike {
  readonly reload = vi.fn();
  private readonly onLoad: (() => void)[] = [];
  private readonly onShow = new Set<() => void>();

  constructor(private loaded = true) {}

  afterLoad = (callback: () => void): void => {
    if (this.loaded) callback();
    else this.onLoad.push(callback);
  };

  onVisible = (callback: () => void): (() => void) => {
    this.onShow.add(callback);
    return () => this.onShow.delete(callback);
  };

  finishLoading(): void {
    this.loaded = true;
    for (const callback of this.onLoad) callback();
  }

  show(): void {
    for (const callback of this.onShow) callback();
  }
}

const yes = (): Promise<boolean> => Promise.resolve(true);
const no = (): Promise<boolean> => Promise.resolve(false);

/** Let the registration and the work after it settle. */
const settle = async (): Promise<void> => {
  await vi.advanceTimersByTimeAsync(0);
};

let container: FakeContainer;
let page: FakePage;
let update: AppUpdate;
let stop: () => void;
let logged: MockInstance<typeof console.error>;

beforeEach(() => {
  vi.useFakeTimers();
  logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
  container = new FakeContainer();
  page = new FakePage();
  update = new AppUpdate();
});

afterEach(() => {
  stop();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function start(): Promise<void> {
  stop = update.start(container, page);
  await settle();
}

describe("registering the worker", () => {
  it("waits for the page to finish loading", async () => {
    page = new FakePage(false);
    await start();
    expect(container.register).not.toHaveBeenCalled();

    page.finishLoading();
    await settle();
    expect(container.register).toHaveBeenCalledExactlyOnceWith(WORKER_URL);
  });

  it("logs a registration that fails, and checks for no updates after it", async () => {
    container.register.mockRejectedValue(new Error("the script is behind a redirect"));
    await start();
    expect(logged).toHaveBeenCalledWith(
      "Couldn't register the service worker, so the app won't open offline.",
      expect.any(Error),
    );
    await vi.advanceTimersByTimeAsync(60 * 60_000);
    page.show();
    expect(container.registration.update).not.toHaveBeenCalled();
    expect(update.ready).toBe(false);
  });
});

describe("update ready", () => {
  it("is not shown for the first worker, which replaces nothing", async () => {
    await start();
    const worker = container.registration.foundUpdate();
    container.takeControl();
    expect(worker.state).toBe("installed");
    expect(update.ready).toBe(false);
  });

  it("is shown once a rebuilt app's worker is installed and waiting under a controlled page", async () => {
    container.controller = {};
    await start();
    expect(update.ready).toBe(false);
    container.registration.foundUpdate();
    expect(update.ready).toBe(true);
  });

  it("waits for the worker to finish installing", async () => {
    container.controller = {};
    await start();
    const worker = new FakeWorker();
    container.registration.installing = worker;
    container.registration.dispatchEvent(new Event("updatefound"));
    expect(update.ready).toBe(false);
    container.registration.waiting = worker;
    worker.finishInstalling();
    expect(update.ready).toBe(true);
  });

  it("is shown at once when a worker was already waiting as the page opened", async () => {
    container.controller = {};
    container.registration.waiting = new FakeWorker();
    await start();
    expect(update.ready).toBe(true);
  });

  it("is not shown on a page no worker controls, such as a hard reload", async () => {
    container.registration.waiting = new FakeWorker();
    await start();
    expect(update.ready).toBe(false);
  });

  it("is shown when another window of the app took over and left this page on the old files", async () => {
    container.controller = {};
    await start();
    container.takeControl();
    expect(update.ready).toBe(true);
  });

  it("can be put away for the page", async () => {
    container.controller = {};
    container.registration.waiting = new FakeWorker();
    await start();
    expect(update.dismissed).toBe(false);
    update.dismiss();
    expect(update.dismissed).toBe(true);
    expect(update.ready).toBe(true);
  });
});

describe("Reload", () => {
  async function withWaitingWorker(): Promise<FakeWorker> {
    container.controller = {};
    const worker = new FakeWorker();
    container.registration.waiting = worker;
    await start();
    return worker;
  }

  it("asks first, and leaves everything as it is when the person keeps their work", async () => {
    const worker = await withWaitingWorker();
    await update.apply(no);
    expect(worker.posted).toEqual([]);
    expect(page.reload).not.toHaveBeenCalled();
    expect(update.applying).toBe(false);
    expect(update.ready).toBe(true);
  });

  it("lets the waiting worker take over, then reloads", async () => {
    const worker = await withWaitingWorker();
    const applying = update.apply(yes);
    await settle();
    expect(worker.posted).toEqual([SKIP_WAITING_MESSAGE]);
    expect(update.applying).toBe(true);
    expect(page.reload).not.toHaveBeenCalled();

    container.takeControl();
    await applying;
    expect(page.reload).toHaveBeenCalledOnce();
    // This page asked for the takeover, so it is not left with a second notice.
    expect(update.ready).toBe(true);
  });

  it("reloads anyway when the worker is slow to take over", async () => {
    await withWaitingWorker();
    const applying = update.apply(yes);
    await vi.advanceTimersByTimeAsync(4000);
    expect(page.reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1500);
    await applying;
    expect(page.reload).toHaveBeenCalledOnce();
  });

  it("only reloads when another window already took over", async () => {
    container.controller = {};
    await start();
    container.takeControl();
    await update.apply(yes);
    expect(page.reload).toHaveBeenCalledOnce();
  });

  it("runs once however many times it is pressed", async () => {
    const worker = await withWaitingWorker();
    const first = update.apply(yes);
    const second = update.apply(yes);
    await settle();
    container.takeControl();
    await Promise.all([first, second]);
    expect(worker.posted).toHaveLength(1);
    expect(page.reload).toHaveBeenCalledOnce();
  });
});

describe("looking for a rebuilt app", () => {
  it("asks the server each time the page becomes visible", async () => {
    await start();
    page.show();
    page.show();
    expect(container.registration.update).toHaveBeenCalledTimes(2);
  });

  it("asks every ten minutes while the page stays open", async () => {
    await start();
    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(container.registration.update).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(container.registration.update).toHaveBeenCalledTimes(2);
  });

  it("logs a run of failed checks once, and again after one succeeds", async () => {
    await start();
    const { update: check } = container.registration;
    check.mockRejectedValue(new TypeError("Failed to fetch"));
    page.show();
    page.show();
    await settle();
    expect(logged).toHaveBeenCalledTimes(1);

    check.mockResolvedValue(undefined);
    page.show();
    await settle();
    check.mockRejectedValue(new TypeError("Failed to fetch"));
    page.show();
    await settle();
    expect(logged).toHaveBeenCalledTimes(2);
  });

  it("stops when told to", async () => {
    await start();
    stop();
    page.show();
    await vi.advanceTimersByTimeAsync(60 * 60_000);
    expect(container.registration.update).not.toHaveBeenCalled();
  });
});
