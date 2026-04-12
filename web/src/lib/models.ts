// ── Model Fetcher ───────────────────────────────────────────────────
//
// Cached model list fetcher for provider API dropdowns.

import { fetchProviderModels } from "./api";

export interface ModelEntry {
  id: string;
  name: string;
}

interface FetchResult {
  models: ModelEntry[];
  error: string | null;
}

export const FALLBACK_MODELS: Record<string, ModelEntry[]> = {
  anthropic: [
    { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
    { id: "claude-haiku-4-5", name: "Claude Haiku 4.5" },
    { id: "claude-opus-4-6", name: "Claude Opus 4.6" },
  ],
  openai: [
    { id: "gpt-4o", name: "gpt-4o" },
    { id: "gpt-4o-mini", name: "gpt-4o-mini" },
    { id: "o3-mini", name: "o3-mini" },
  ],
  gemini: [
    { id: "gemini-2.5-pro", name: "Gemini 2.5 Pro" },
    { id: "gemini-2.5-flash", name: "Gemini 2.5 Flash" },
    { id: "gemini-2.0-flash", name: "Gemini 2.0 Flash" },
  ],
  ollama: [
    { id: "llama3.1", name: "llama3.1" },
    { id: "mistral", name: "mistral" },
    { id: "qwen2.5", name: "qwen2.5" },
  ],
};

export const DEFAULT_MODELS: Record<string, string> = {
  anthropic: "claude-sonnet-4-6",
  openai: "gpt-4o",
  gemini: "gemini-2.5-flash",
  ollama: "llama3.1",
};

export const DEFAULT_EMBEDDING_MODELS: Record<string, string> = {
  openai: "text-embedding-3-small",
  gemini: "gemini-embedding-001",
  ollama: "nomic-embed-text",
};

// Hardcoded embedding model lists — provider APIs don't list embedding models
export const EMBEDDING_MODEL_LISTS: Record<string, ModelEntry[]> = {
  openai: [
    { id: "text-embedding-3-small", name: "text-embedding-3-small" },
    { id: "text-embedding-3-large", name: "text-embedding-3-large" },
    { id: "text-embedding-ada-002", name: "text-embedding-ada-002" },
  ],
  gemini: [{ id: "gemini-embedding-001", name: "gemini-embedding-001" }],
  ollama: [
    { id: "nomic-embed-text", name: "nomic-embed-text" },
    { id: "mxbai-embed-large", name: "mxbai-embed-large" },
    { id: "all-minilm", name: "all-minilm" },
  ],
};

export const EMBEDDING_PROVIDERS = ["openai", "gemini", "ollama"];

const cache = new Map<string, FetchResult>();

function cacheKey(provider: string, apiKey?: string, url?: string): string {
  return `${provider}:${apiKey ?? ""}:${url ?? ""}`;
}

export async function fetchModels(
  provider: string,
  apiKey?: string,
  url?: string,
): Promise<FetchResult> {
  const key = cacheKey(provider, apiKey, url);
  const cached = cache.get(key);
  if (cached) return cached;

  try {
    const data = await fetchProviderModels(provider, apiKey, url);
    if (data.models.length > 0) {
      const result: FetchResult = { models: data.models, error: null };
      cache.set(key, result);
      return result;
    }
    return {
      models: FALLBACK_MODELS[provider] ?? [],
      error: data.error ?? "no models returned",
    };
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      models: FALLBACK_MODELS[provider] ?? [],
      error: message,
    };
  }
}

export function invalidateProvider(provider: string): void {
  for (const key of cache.keys()) {
    if (key.startsWith(provider + ":")) {
      cache.delete(key);
    }
  }
}

export function invalidateAll(): void {
  cache.clear();
}

export function debounce<A extends unknown[]>(
  fn: (...args: A) => void,
  ms: number,
): (...args: A) => void {
  let timer: ReturnType<typeof setTimeout>;
  return function (...args: A): void {
    clearTimeout(timer);
    timer = setTimeout(() => {
      fn(...args);
    }, ms);
  };
}
