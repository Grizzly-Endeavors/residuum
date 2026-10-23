// Sending a composer line that may be a command: one whose first word is `/`
// and the name of one of the actions the composer offers runs that action,
// with the rest of the line as its text; anything else, a pasted path like
// `/home/bear/app.log has the error` included, is a message.

import { actionRegistry, readCommandLine, type AppAction } from "./action-registry.svelte";
import { notifications } from "./notifications.svelte";

/** What became of a line: sent on as a message, run as an action, or refused. */
export type CommandLineOutcome = "message" | "ran" | "refused";

/**
 * Run `line` when it names one of `actions`. An action that can't run now
 * says why in a toast and gives `refused`, so the composer keeps the line;
 * a line that isn't a command gives `message`, for the caller to send.
 */
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
