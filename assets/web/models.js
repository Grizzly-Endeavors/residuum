// ── Residuum Web UI — Model Fetcher ──────────────────────────────────
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

    defaultEmbeddingModels: {
        openai: 'text-embedding-3-small',
        gemini: 'gemini-embedding-001',
        ollama: 'nomic-embed-text'
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
     * Appends an "Other..." option that reveals a text input for custom model IDs.
     * Optional `defaultOverride` replaces the built-in default for this provider.
     */
    async populateSelect(selectEl, provider, apiKey, url, currentValue, defaultOverride) {
        if (!selectEl) return;

        // Show loading state
        selectEl.innerHTML = '<option value="">Loading models...</option>';
        selectEl.disabled = true;
        this._hideOtherInput(selectEl);

        const result = await this.fetch(provider, apiKey, url);

        selectEl.innerHTML = '';
        if (result.models.length === 0) {
            selectEl.innerHTML = '<option value="">No models available</option>';
            selectEl.disabled = false;
            this._appendOtherOption(selectEl);
            return;
        }

        const defaultModel = defaultOverride !== undefined ? defaultOverride : this.defaultModels[provider];
        const selected = currentValue || defaultModel || '';

        let foundInList = false;
        for (const model of result.models) {
            const opt = document.createElement('option');
            opt.value = model.id;
            opt.textContent = model.name || model.id;
            if (model.id === selected) {
                opt.selected = true;
                foundInList = true;
            }
            selectEl.appendChild(opt);
        }

        this._appendOtherOption(selectEl);

        // If the current value isn't in the list, activate "Other" with it pre-filled
        if (selected && !foundInList) {
            selectEl.value = '__other__';
            this._showOtherInput(selectEl, selected);
        }

        selectEl.disabled = false;
    },

    /** Append the "Other..." option and wire up its toggle behavior. */
    _appendOtherOption(selectEl) {
        const opt = document.createElement('option');
        opt.value = '__other__';
        opt.textContent = 'Other...';
        selectEl.appendChild(opt);

        // Avoid double-binding
        if (selectEl._otherBound) return;
        selectEl._otherBound = true;

        selectEl.addEventListener('change', () => {
            if (selectEl.value === '__other__') {
                this._showOtherInput(selectEl, '');
            } else {
                this._hideOtherInput(selectEl);
            }
        });
    },

    /** Show the custom model text input below the select. */
    _showOtherInput(selectEl, prefill) {
        const wrap = selectEl.closest('.model-select-wrap') || selectEl.parentElement;
        let input = wrap.querySelector('.model-other-input');
        if (!input) {
            input = document.createElement('input');
            input.type = 'text';
            input.className = 'model-other-input';
            input.placeholder = 'Enter model ID...';
            wrap.appendChild(input);
        }
        input.value = prefill || '';
        input.style.display = '';
        input.focus();
    },

    /** Hide the custom model text input. */
    _hideOtherInput(selectEl) {
        const wrap = selectEl.closest('.model-select-wrap') || selectEl.parentElement;
        const input = wrap.querySelector('.model-other-input');
        if (input) {
            input.style.display = 'none';
            input.value = '';
        }
    },

    /**
     * Get the effective model ID from a select that may have "Other" active.
     * Use this instead of reading selectEl.value directly.
     */
    getSelectedModel(selectEl) {
        if (!selectEl) return '';
        if (selectEl.value === '__other__') {
            const wrap = selectEl.closest('.model-select-wrap') || selectEl.parentElement;
            const input = wrap.querySelector('.model-other-input');
            return input ? input.value.trim() : '';
        }
        return selectEl.value;
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
