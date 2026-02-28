// ── IronClaw Web UI — Model Fetcher ──────────────────────────────────
//
// Shared utility for fetching available models from provider APIs.
// Used by both setup.js and settings.js to populate model dropdowns.

const ModelFetcher = {
    cache: {},

    // Hardcoded fallbacks when API call fails
    fallbacks: {
        anthropic: [
            { id: 'claude-sonnet-4-6', name: 'Claude Sonnet 4.6' },
            { id: 'claude-haiku-4-5', name: 'Claude Haiku 4.5' },
            { id: 'claude-opus-4-6', name: 'Claude Opus 4.6' }
        ],
        openai: [
            { id: 'gpt-4o', name: 'gpt-4o' },
            { id: 'gpt-4o-mini', name: 'gpt-4o-mini' },
            { id: 'o3-mini', name: 'o3-mini' }
        ],
        gemini: [
            { id: 'gemini-2.5-pro', name: 'Gemini 2.5 Pro' },
            { id: 'gemini-2.5-flash', name: 'Gemini 2.5 Flash' },
            { id: 'gemini-2.0-flash', name: 'Gemini 2.0 Flash' }
        ],
        ollama: [
            { id: 'llama3.1', name: 'llama3.1' },
            { id: 'mistral', name: 'mistral' },
            { id: 'qwen2.5', name: 'qwen2.5' }
        ]
    },

    defaultModels: {
        anthropic: 'claude-sonnet-4-6',
        openai: 'gpt-4o',
        gemini: 'gemini-2.5-flash',
        ollama: 'llama3.1'
    },

    /** Cache key for a provider+key+url combo. */
    _cacheKey(provider, apiKey, url) {
        return `${provider}:${apiKey || ''}:${url || ''}`;
    },

    /**
     * Fetch models from the backend proxy.
     * Returns { models: [{id, name}], error: string|null }.
     * Results are cached per provider+key+url.
     */
    async fetch(provider, apiKey, url) {
        const key = this._cacheKey(provider, apiKey, url);
        if (this.cache[key]) return this.cache[key];

        try {
            const body = { provider };
            if (apiKey) body.api_key = apiKey;
            if (url) body.url = url;

            const resp = await fetch('/api/providers/models', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body)
            });
            const data = await resp.json();

            if (data.models && data.models.length > 0) {
                const result = { models: data.models, error: null };
                this.cache[key] = result;
                return result;
            }

            // API returned no models or an error — use fallbacks
            return {
                models: this.fallbacks[provider] || [],
                error: data.error || 'no models returned'
            };
        } catch (err) {
            return {
                models: this.fallbacks[provider] || [],
                error: err.message
            };
        }
    },

    /** Invalidate cache entries for a given provider (all keys). */
    invalidate(provider) {
        for (const key of Object.keys(this.cache)) {
            if (key.startsWith(provider + ':')) {
                delete this.cache[key];
            }
        }
    },

    /** Invalidate all cache entries. */
    invalidateAll() {
        this.cache = {};
    },

    /**
     * Populate a <select> element with models from the given provider.
     * Shows a loading state while fetching. Selects `currentValue` if present.
     */
    async populateSelect(selectEl, provider, apiKey, url, currentValue) {
        if (!selectEl) return;

        // Show loading state
        selectEl.innerHTML = '<option value="">Loading models...</option>';
        selectEl.disabled = true;

        const result = await this.fetch(provider, apiKey, url);

        selectEl.innerHTML = '';
        if (result.models.length === 0) {
            selectEl.innerHTML = '<option value="">No models available</option>';
            selectEl.disabled = false;
            return;
        }

        const defaultModel = this.defaultModels[provider];
        const selected = currentValue || defaultModel || '';

        for (const model of result.models) {
            const opt = document.createElement('option');
            opt.value = model.id;
            opt.textContent = model.name || model.id;
            if (model.id === selected) opt.selected = true;
            selectEl.appendChild(opt);
        }

        // If nothing was selected and we have a value, add it as an option
        if (selected && !selectEl.value) {
            const opt = document.createElement('option');
            opt.value = selected;
            opt.textContent = selected;
            opt.selected = true;
            selectEl.prepend(opt);
        }

        selectEl.disabled = false;
    },

    /**
     * Create a debounced version of a function.
     * Used to debounce API key input before fetching models.
     */
    debounce(fn, ms) {
        let timer;
        return function (...args) {
            clearTimeout(timer);
            timer = setTimeout(() => fn.apply(this, args), ms);
        };
    }
};
