import { chatRoutes } from "./chat";
import { configRoutes } from "./config";
import { lifecycleRoutes } from "./lifecycle";
import type { Route } from "./routes";
import { sessionRoutes } from "./sessions";
import { workspaceRoutes } from "./workspace";

/** Every route table the mock serves. Their patterns don't overlap, so the order doesn't matter. */
export const apiRoutes: readonly Route[] = [
  ...lifecycleRoutes,
  ...sessionRoutes,
  ...configRoutes,
  ...chatRoutes,
  ...workspaceRoutes,
];
