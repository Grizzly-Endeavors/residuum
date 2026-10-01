// A component whose code loads the first time something needs it: the
// Settings modal, the command palette, the file editor and the setup wizard
// are built into chunks of their own, so the shell, Home and Chat start
// without them (design §11).

import { untrack, type Component } from "svelte";
import { userErrorMessage } from "./errors";
import { notifications } from "./notifications.svelte";

type Loader<Props extends object> = () => Promise<{ default: Component<Props> }>;

export class LazyComponent<Props extends object> {
  /** The component, once its code has loaded. It stays for the life of the page. */
  component = $state.raw<Component<Props> | null>(null);
  /** Whether the code is being fetched now. */
  loading = $state(false);
  /** Whether the last attempt to load the code failed, until the next attempt starts. */
  failed = $state(false);

  readonly #load: Loader<Props>;
  readonly #what: string;

  /** `what` names the feature to the person if its code can't be fetched: "Settings". */
  constructor(load: Loader<Props>, what: string) {
    this.#load = load;
    this.#what = what;
  }

  /**
   * Start fetching the code, once. A failure (the connection dropped, or the
   * app was rebuilt since this page loaded) is shown to the person, calls
   * `onFailure` so the caller can put away whatever asked for the component,
   * and leaves the next call free to try again.
   */
  ensure(onFailure?: () => void): void {
    // Effects call this, and they follow what asked for the component, not the component's own state.
    if (untrack(() => this.component !== null || this.loading)) return;
    this.loading = true;
    this.failed = false;
    this.#load().then(
      (module) => {
        this.loading = false;
        this.component = module.default;
      },
      (error: unknown) => {
        this.loading = false;
        this.failed = true;
        // A page left open across an update asks for chunk names that no longer exist, so a retry alone won't help.
        const reason = userErrorMessage(error, { action: `Couldn't open ${this.#what}.` });
        notifications.surface("error", `${reason} If it keeps happening, reload the page.`);
        onFailure?.();
      },
    );
  }
}
