// The composer's model and thinking control: what an agent's main model and
// its thinking level are, the models its provider offers, and changing either
// through the config write coordinator. The value follows `providers.toml`, so
// a change made in Settings, by a restore, or outside the page shows here.

import { agentConfigFile, configCoordinator, type ConfigFile } from "../../lib/config-coordinator";
import { userErrorMessage } from "../../lib/errors";
import { providerOptions, providerTypeLabel, splitModel } from "../../lib/model-roles";
import { fetchModels, type ModelEntry } from "../../lib/models";
import { notifications } from "../../lib/notifications.svelte";
import { modelRoleJson, parseProvidersToml } from "../../lib/settings-toml";
import type { SettingsModelAssignments } from "../../lib/types";

/** The levels the composer offers. None chosen leaves thinking to the agent's default. */
export const COMPOSER_THINKING: readonly { value: string; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

/** The trigger's words: the model, and the thinking level when one is set. */
export function modelControlLabel(model: string, thinking: string): string {
  if (thinking === "") return model;
  return thinking === "off" ? `${model}, no extra thinking` : `${model}, ${thinking} thinking`;
}

export class MainModel {
  /** The main role's `provider/model`, or "" while unread or unset. */
  value = $state("");
  thinking = $state("");
  loaded = $state(false);
  loadError = $state<string | null>(null);
  /** The main provider's models, or common ones when its list can't be read. */
  models = $state<ModelEntry[]>([]);
  listError = $state<string | null>(null);
  providerLabel = $state("");
  /** Writes under way. The coordinator runs them one at a time, each on the file as the last left it. */
  private writes = $state(0);

  private readonly source = Symbol("composer model control");
  /** Sequences reads, so a slow one can't overwrite what a later one found. */
  private reads = 0;

  /**
   * @param onWritten Runs after a change is written, so the agent takes it up
   *   from its next reply.
   */
  constructor(
    readonly agent: string,
    private readonly onWritten: () => void,
  ) {}

  get model(): string {
    return splitModel(this.value).model;
  }

  get saving(): boolean {
    return this.writes > 0;
  }

  /** The model's name as its provider lists it, or its id. */
  get modelName(): string {
    const model = this.model;
    return this.models.find((entry) => entry.id === model)?.name || model;
  }

  private get file(): ConfigFile {
    return agentConfigFile(this.agent, "providers");
  }

  /** Read the file now and after every change made elsewhere. Returns the function that stops following. */
  follow(): () => void {
    void this.load();
    return configCoordinator.subscribe(this.file, (change) => {
      if (change.source !== this.source) void this.load();
    });
  }

  async load(): Promise<void> {
    const read = ++this.reads;
    let raw: string;
    try {
      raw = await configCoordinator.read(this.file);
    } catch (err) {
      if (read !== this.reads) return;
      this.loadError = userErrorMessage(err, { action: `Couldn't read ${this.agent}'s model.` });
      this.loaded = true;
      return;
    }
    if (read === this.reads) await this.show(raw, read);
  }

  private async show(raw: string, read: number): Promise<void> {
    const parsed = parseProvidersToml(raw);
    this.value = parsed.models.main;
    this.thinking = parsed.models.overrides.main?.thinking ?? "";
    this.loadError = null;
    this.loaded = true;
    const { provider } = splitModel(this.value);
    const option = providerOptions(parsed.providers, "main").find((o) => o.name === provider);
    this.providerLabel =
      option === undefined ? provider : (option.entry?.name ?? providerTypeLabel(option.type));
    if (option === undefined) {
      this.models = [];
      this.listError = null;
      return;
    }
    const { apiKey, url } = option.entry ?? { apiKey: "", url: "" };
    const list = await fetchModels(this.agent, option.type, apiKey || undefined, url || undefined);
    if (read !== this.reads) return;
    this.models = list.models;
    this.listError = list.error;
  }

  /** Make `modelId`, from the main provider, the main model. Its failover list and thinking stay. */
  choose(modelId: string): Promise<void> {
    if (modelId === this.model) return Promise.resolve();
    const { provider } = splitModel(this.value);
    return this.write(
      (models) =>
        modelRoleJson(`${provider}/${modelId}`, models.overrides.main, models.fallbacks.main),
      "Couldn't switch the model.",
    );
  }

  /** Set the thinking level; choosing the level already set clears it. */
  toggleThinking(level: string): Promise<void> {
    const next = level === this.thinking ? "" : level;
    return this.write((models) => {
      const overrides = { temperature: models.overrides.main?.temperature ?? "", thinking: next };
      return modelRoleJson(models.main, overrides, models.fallbacks.main);
    }, "Couldn't change the thinking level.");
  }

  /** Write the main role, built from the file as it is now so nothing else in it is lost. */
  private async write(
    buildMain: (models: SettingsModelAssignments) => unknown,
    failure: string,
  ): Promise<void> {
    this.writes += 1;
    try {
      const saved = await configCoordinator.edit(
        this.file,
        (raw) => {
          const { models } = parseProvidersToml(raw);
          return models.main === "" ? null : { models: { main: buildMain(models) } };
        },
        this.source,
      );
      if (!saved.result.valid) {
        notifications.surface(
          "error",
          `${failure} Residuum didn't accept the change. Open Model settings to see why.`,
          saved.result.error,
        );
        return;
      }
      if (saved.written) this.onWritten();
      if (saved.raw !== null) await this.show(saved.raw, ++this.reads);
    } catch (err) {
      notifications.surface("error", userErrorMessage(err, { action: failure }));
    } finally {
      this.writes -= 1;
    }
  }
}
