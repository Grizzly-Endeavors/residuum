import { actionRegistry, readCommandLine, type AppAction } from "./action-registry.svelte";
import { notifications } from "./notifications.svelte";

export type CommandLineOutcome = "message" | "ran" | "refused";

export function runCommandLine(actions: readonly AppAction[], line: string): CommandLineOutcome {
  const command = readCommandLine(actions, line);
  if (command === null) return "message";
  const reason =
    command.text === ""
      ? command.action.disabled
      : (command.action.disabledWithText ?? command.action.disabled);
  if (reason !== undefined) {
    notifications.surface("error", `Couldn't run /${command.action.command ?? ""}: ${reason}.`);
    return "refused";
  }
  void actionRegistry.run(command.action, command.text);
  return "ran";
}
