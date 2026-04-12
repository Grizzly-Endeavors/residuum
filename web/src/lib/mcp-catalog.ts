import type { McpCatalogEntry } from "./types";
import { fetchMcpCatalog } from "./api";

let catalog: McpCatalogEntry[] | null = null;
let pending: Promise<McpCatalogEntry[]> | null = null;

export function loadCatalog(): Promise<McpCatalogEntry[]> {
  if (catalog !== null) return Promise.resolve(catalog);
  if (pending !== null) return pending;
  pending = fetchMcpCatalog().then((result) => {
    catalog = result;
    pending = null;
    return result;
  });
  return pending;
}
