import { agentInboxRoutes } from "./agent-inbox";
import { checkpointRoutes } from "./checkpoints";
import { chatRoutes } from "./chat";
import { configRoutes } from "./config";
import { controlRoutes } from "./controls";
import { hubInboxRoutes } from "./hub-inbox";
import { inboxRoutes } from "./inbox";
import { lifecycleRoutes } from "./lifecycle";
import { modelRoutes } from "./model";
import type { Route } from "./routes";
import { scheduledRoutes } from "./scheduled";
import { sessionRoutes } from "./sessions";
import { teamEventRoutes } from "./team-events";
import { updateRoutes } from "./update";
import { workbenchRoutes } from "./workbench";
import { workspaceRoutes } from "./workspace";

/** Every route table the mock serves. Their patterns don't overlap, so the order of the tables doesn't matter; within one, the first match wins. */
export const apiRoutes: readonly Route[] = [
  ...lifecycleRoutes,
  ...sessionRoutes,
  ...configRoutes,
  ...chatRoutes,
  ...inboxRoutes,
  ...hubInboxRoutes,
  ...teamEventRoutes,
  ...agentInboxRoutes,
  ...scheduledRoutes,
  ...updateRoutes,
  ...checkpointRoutes,
  ...workspaceRoutes,
  ...workbenchRoutes,
  ...modelRoutes,
  ...controlRoutes,
];
