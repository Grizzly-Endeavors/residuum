import { untrack, type Component } from "svelte";
import { userErrorMessage } from "./errors";
import { notifications } from "./notifications.svelte";

type Loader<Props extends object> = () => Promise<{ default: Component<Props> }>;

export class LazyComponent<Props extends object> {
  component = $state.raw<Component<Props> | null>(null);
  loading = $state(false);
  failed = $state(false);

  readonly #load: Loader<Props>;
  readonly #what: string;

  constructor(load: Loader<Props>, what: string) {
    this.#load = load;
    this.#what = what;
  }

  ensure(onFailure?: () => void): void {
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
        const reason = userErrorMessage(error, { action: `Couldn't open ${this.#what}.` });
        notifications.surface("error", `${reason} If it keeps happening, reload the page.`);
        onFailure?.();
      },
    );
  }
}
