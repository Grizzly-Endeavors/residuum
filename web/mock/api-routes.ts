import { chatRoutes } from "./chat";
import { configRoutes } from "./config";
import { controlRoutes } from "./controls";
import { hubInboxRoutes } from "./hub-inbox";
import { inboxRoutes } from "./inbox";
import { lifecycleRoutes } from "./lifecycle";
import { modelRoutes } from "./model";
import type { Route } from "./routes";
import { sessionRoutes } from "./sessions";
import { workbenchRoutes } from "./workbench";
import { workspaceRoutes } from "./workspace";

/** Every route table the mock serves. Their patterns don't overlap, so the order doesn't matter. */
export const apiRoutes: readonly Route[] = [
  ...lifecycleRoutes,
  ...sessionRoutes,
  ...configRoutes,
  ...chatRoutes,
  ...inboxRoutes,
  ...hubInboxRoutes,
  ...workspaceRoutes,
  ...workbenchRoutes,
  ...modelRoutes,
  ...controlRoutes,
];
