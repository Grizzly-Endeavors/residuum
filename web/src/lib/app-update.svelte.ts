import { SKIP_WAITING_MESSAGE, WORKER_URL } from "../sw/protocol";
import { userErrorReason } from "./errors";

const UPDATE_CHECK_MS = 10 * 60_000;

const TAKEOVER_TIMEOUT_MS = 5000;

export interface WorkerLike extends EventTarget {
  readonly state: string;
  postMessage: (message: unknown) => void;
}

export interface RegistrationLike extends EventTarget {
  readonly waiting: WorkerLike | null;
  readonly installing: WorkerLike | null;
  update: () => Promise<unknown>;
}

export interface ContainerLike extends EventTarget {
  readonly controller: unknown;
  /** Resolves to no registration when the browser blocks service workers, as Playwright's `serviceWorkers: "block"` does. */
  register: (url: string) => Promise<RegistrationLike | undefined>;
}

export interface PageLike {
  afterLoad: (callback: () => void) => void;
  onVisible: (callback: () => void) => () => void;
  reload: () => void;
}

export function browserPage(): PageLike {
  return {
    afterLoad: (callback) => {
      if (document.readyState === "complete") callback();
      else window.addEventListener("load", callback, { once: true });
    },
    onVisible: (callback) => {
      const listener = (): void => {
        if (document.visibilityState === "visible") callback();
      };
      document.addEventListener("visibilitychange", listener);
      return () => {
        document.removeEventListener("visibilitychange", listener);
      };
    },
    reload: () => {
      window.location.reload();
    },
  };
}

function recordFailure(action: string, error: unknown): void {
  userErrorReason(error, { action });
}

export class AppUpdate {
  ready = $state(false);
  applying = $state(false);
  dismissed = $state(false);

  #container: ContainerLike | null = null;
  #page: PageLike | null = null;
  #registration: RegistrationLike | null = null;
  #hadController = false;
  #checkFailing = false;
  #reloading = false;

  start(container: ContainerLike, page: PageLike): () => void {
    this.#container = container;
    this.#page = page;
    this.#hadController = container.controller !== null;

    const onControllerChange = (): void => {
      if (!this.#hadController) this.#hadController = true;
      else if (!this.applying) this.ready = true;
    };
    container.addEventListener("controllerchange", onControllerChange);

    let stopped = false;
    let stopVisible = (): void => undefined;
    let timer: ReturnType<typeof setInterval> | undefined;
    page.afterLoad(() => {
      if (stopped) return;
      void this.#register().then((registered) => {
        if (!registered || stopped) return;
        stopVisible = page.onVisible(() => void this.#check());
        timer = setInterval(() => void this.#check(), UPDATE_CHECK_MS);
      });
    });

    return () => {
      stopped = true;
      container.removeEventListener("controllerchange", onControllerChange);
      stopVisible();
      clearInterval(timer);
    };
  }

  dismiss(): void {
    this.dismissed = true;
  }

  async apply(confirm: () => Promise<boolean>): Promise<void> {
    const container = this.#container;
    const page = this.#page;
    if (this.#reloading || container === null || page === null) return;
    this.#reloading = true;
    if (!(await confirm())) {
      this.#reloading = false;
      return;
    }
    this.applying = true;
    const waiting = this.#registration?.waiting ?? null;
    if (waiting !== null) {
      const tookOver = new Promise<void>((resolve) => {
        container.addEventListener(
          "controllerchange",
          () => {
            resolve();
          },
          { once: true },
        );
      });
      const timedOut = new Promise<void>((resolve) => setTimeout(resolve, TAKEOVER_TIMEOUT_MS));
      waiting.postMessage(SKIP_WAITING_MESSAGE);
      await Promise.race([tookOver, timedOut]);
    }
    page.reload();
  }

  async #register(): Promise<boolean> {
    if (this.#container === null) return false;
    try {
      const registration = await this.#container.register(WORKER_URL);
      if (registration === undefined) return false;
      this.#registration = registration;
      registration.addEventListener("updatefound", () => {
        this.#watchInstall(registration.installing);
      });
      this.#watchInstall(registration.installing);
      this.#markReady();
      return true;
    } catch (error) {
      recordFailure("Couldn't register the service worker, so the app won't open offline.", error);
      return false;
    }
  }

  #watchInstall(worker: WorkerLike | null): void {
    worker?.addEventListener("statechange", () => {
      if (worker.state === "installed") this.#markReady();
    });
  }

  #markReady(): void {
    const waiting = this.#registration?.waiting ?? null;
    if (waiting !== null && this.#container?.controller !== null) this.ready = true;
  }

  async #check(): Promise<void> {
    try {
      await this.#registration?.update();
      this.#checkFailing = false;
    } catch (error) {
      if (!this.#checkFailing) recordFailure("Couldn't check for an app update.", error);
      this.#checkFailing = true;
    }
  }
}

export const appUpdate = new AppUpdate();

export function startServiceWorker(): void {
  if (!__SERVICE_WORKER__ || !("serviceWorker" in navigator)) return;
  appUpdate.start(navigator.serviceWorker, browserPage());
}
