// The app's service worker as the page sees it: registering it, noticing that
// the app was rebuilt, and switching to the new version when the person says
// (design §11). The worker itself is `src/sw/worker.ts`.
//
// A rebuilt app installs a new worker beside the active one, and it waits: it
// takes over only when asked, so the files under an open page never change on
// their own. "Update ready" shows while one waits, and Reload asks the waiting
// worker to take over, then reloads the page.

import { SKIP_WAITING_MESSAGE, WORKER_URL } from "../sw/protocol";
import { userErrorReason } from "./errors";

/** How often a page left open asks the server whether the app was rebuilt. */
const UPDATE_CHECK_MS = 10 * 60_000;

/** How long Reload waits for the new worker to take over before reloading regardless. */
const TAKEOVER_TIMEOUT_MS = 5000;

/** The parts of a service worker the page uses. */
export interface WorkerLike extends EventTarget {
  readonly state: string;
  postMessage: (message: unknown) => void;
}

/** The parts of a service worker registration the page uses. */
export interface RegistrationLike extends EventTarget {
  readonly waiting: WorkerLike | null;
  readonly installing: WorkerLike | null;
  update: () => Promise<unknown>;
}

/** The parts of `navigator.serviceWorker` the page uses. */
export interface ContainerLike extends EventTarget {
  readonly controller: unknown;
  register: (url: string) => Promise<RegistrationLike>;
}

/** What the page lends the worker's bookkeeping: its lifecycle, and reloading it. */
export interface PageLike {
  /** Run `callback` once the page has loaded, so installing the worker never competes with loading the app. */
  afterLoad: (callback: () => void) => void;
  /** Call `callback` each time the page becomes visible. Returns a function that stops. */
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

/**
 * Log a failure the person has no message for: a worker that fails to install
 * or update costs nothing until the app is opened offline or the next rebuild.
 * The helper that words errors for people is also where raw errors reach the
 * console, so the reason it returns goes unused.
 */
function recordFailure(action: string, error: unknown): void {
  userErrorReason(error, { action });
}

export class AppUpdate {
  /**
   * A rebuilt app is installed and waiting, or took over in another window of
   * this app and left this page running the old files. Reload gets the new one.
   */
  ready = $state(false);
  /** Reload was chosen and is under way. */
  applying = $state(false);
  /** The banner was put away; it comes back with the next page load. */
  dismissed = $state(false);

  #container: ContainerLike | null = null;
  #page: PageLike | null = null;
  #registration: RegistrationLike | null = null;
  /** Whether a worker controlled this page before the latest change of controller. */
  #hadController = false;
  /** Whether the last check for an update failed, so a run of failures is logged once. */
  #checkFailing = false;
  /** Reload is waiting on an answer or under way, so pressing it again starts nothing. */
  #reloading = false;

  /**
   * Register the worker once the page has loaded, and keep looking for a
   * rebuilt app while the page is open. Returns a function that stops
   * looking, for tests.
   */
  start(container: ContainerLike, page: PageLike): () => void {
    this.#container = container;
    this.#page = page;
    this.#hadController = container.controller !== null;

    const onControllerChange = (): void => {
      // The first worker claiming a page that had none replaces nothing.
      if (!this.#hadController) this.#hadController = true;
      // Otherwise another window of the app reloaded onto a new version: this page still runs the old one.
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

  /** Put the banner away for this page. */
  dismiss(): void {
    this.dismissed = true;
  }

  /**
   * Switch to the new version: ask `confirm` first (it says whether leaving
   * would lose unsaved work), then let the waiting worker take over and reload.
   */
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
      // Reloading under the old worker would still get the new document, so a worker that is slow to take over only delays the reload.
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
      this.#registration = registration;
      registration.addEventListener("updatefound", () => {
        this.#watchInstall(registration.installing);
      });
      this.#watchInstall(registration.installing);
      // A worker installed while the page was closed is already waiting.
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

  /** Update ready, once there is a waiting worker and a page it would replace. */
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

/** Register the service worker, in builds only: the dev server and the mock's dev mode never run one. */
export function startServiceWorker(): void {
  if (!__SERVICE_WORKER__ || !("serviceWorker" in navigator)) return;
  appUpdate.start(navigator.serviceWorker, browserPage());
}
