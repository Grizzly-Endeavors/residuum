// ── IronClaw Web UI — Setup Wizard ────────────────────────────────────
//
// Multi-step wizard for first-run configuration. Generates a valid
// config.toml and POSTs it to /api/config/complete-setup.

const Setup = {
    step: 0,
    totalSteps: 5,

    // Collected state
    timezone: '',
    // Multi-provider: track which providers are selected and their per-provider config
    selectedProviders: ['anthropic'],  // list of selected provider keys
    providerConfigs: {
        anthropic: { apiKey: '', model: '', url: '' },
        openai: { apiKey: '', model: '', url: '' },
        gemini: { apiKey: '', model: '', url: '' },
        ollama: { apiKey: '', model: '', url: '' }
    },
    mainProvider: 'anthropic',  // which selected provider is the "main" one
    // Legacy compat — keep providerType/apiKey/model as derived getters
    get providerType() { return this.mainProvider; },
    get apiKey() { return this.providerConfigs[this.mainProvider]?.apiKey || ''; },
    get model() { return this.providerConfigs[this.mainProvider]?.model || ''; },
    get providerUrl() { return this.providerConfigs[this.mainProvider]?.url || ''; },
    useDefaultRoles: true,
    // Per-role state: { provider, apiKey, url, model }
    roles: {
        observer: { provider: '', apiKey: '', url: '', model: '' },
        reflector: { provider: '', apiKey: '', url: '', model: '' },
        pulse: { provider: '', apiKey: '', url: '', model: '' }
    },

    // Providers that support embedding models
    embeddingProviders: ['openai', 'gemini', 'ollama'],
    mcpServers: [],  // [{name, command, args, env}]
    catalog: [],

    providers: {
        anthropic: {
            name: 'Anthropic',
            desc: 'Claude models (Sonnet, Haiku, Opus)',
            keyEnv: 'ANTHROPIC_API_KEY'
        },
        openai: {
            name: 'OpenAI',
            desc: 'GPT and o-series models',
            keyEnv: 'OPENAI_API_KEY'
        },
        gemini: {
            name: 'Google Gemini',
            desc: 'Gemini models via Google AI',
            keyEnv: 'GEMINI_API_KEY'
        },
        ollama: {
            name: 'Ollama',
            desc: 'Local models (no API key needed)',
            keyEnv: ''
        }
    },

    async init() {
        // Fetch timezone and catalog in parallel
        const [tzRes, catalogRes] = await Promise.all([
            fetch('/api/system/timezone').then(r => r.json()).catch(() => ({ timezone: '' })),
            fetch('/api/mcp-catalog').then(r => r.json()).catch(() => [])
        ]);
        this.timezone = tzRes.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone || '';
        this.catalog = catalogRes;
        this.step = 0;
        this.render();
    },

    render() {
        const card = document.getElementById('setup-card');
        const dots = Array.from({ length: this.totalSteps }, (_, i) => {
            const cls = i < this.step ? 'done' : i === this.step ? 'active' : '';
            return `<div class="step-dot ${cls}"></div>`;
        }).join('');

        const stepIndicator = `<div class="setup-step-indicator">${dots}</div>`;

        switch (this.step) {
            case 0: card.innerHTML = stepIndicator + this.renderWelcome(); break;
            case 1: card.innerHTML = stepIndicator + this.renderProvider(); break;
            case 2: card.innerHTML = stepIndicator + this.renderRoles(); break;
            case 3: card.innerHTML = stepIndicator + this.renderMcp(); break;
            case 4: card.innerHTML = stepIndicator + this.renderReview(); break;
        }
        this.bindStep();
    },

    // ── Step 0: Welcome + Timezone ────────────────────────────────────

    renderWelcome() {
        return `
            <h2>Welcome to IronClaw</h2>
            <p class="subtitle">Let's get your agent configured. This will only take a minute.</p>
            <div class="settings-field">
                <label>Timezone (IANA format)</label>
                <input type="text" id="setup-tz" value="${esc(this.timezone)}"
                    placeholder="America/New_York">
            </div>
            <div class="setup-nav">
                <div></div>
                <button class="btn btn-primary" id="setup-next">Next</button>
            </div>
        `;
    },

    // ── Step 1: Providers ─────────────────────────────────────────────

    renderProvider() {
        const options = Object.entries(this.providers).map(([key, p]) => {
            const isSelected = this.selectedProviders.includes(key);
            return `
            <div class="provider-option ${isSelected ? 'selected' : ''}" data-provider="${key}">
                <div class="provider-check">${isSelected ? '&#10003;' : ''}</div>
                <div>
                    <div class="provider-name">${p.name}</div>
                    <div class="provider-desc">${p.desc}</div>
                </div>
            </div>
            `;
        }).join('');

        // Build per-provider config sections for each selected provider
        const configSections = this.selectedProviders.map(key => {
            const p = this.providers[key];
            const cfg = this.providerConfigs[key];
            const isMain = key === this.mainProvider;

            const keyField = key !== 'ollama' ? `
                <div class="settings-field">
                    <label>API Key${p.keyEnv ? ` (or set ${p.keyEnv} env var)` : ''}</label>
                    <input type="password" class="provider-apikey" data-provider="${key}"
                        value="${esc(cfg.apiKey)}" placeholder="sk-...">
                    <div class="model-error" data-provider-error="${key}" style="display:none"></div>
                </div>
            ` : '';

            const mainRadio = this.selectedProviders.length > 1 ? `
                <label class="provider-main-label">
                    <input type="radio" name="main-provider" value="${key}" ${isMain ? 'checked' : ''}>
                    Use as main provider
                </label>
            ` : '';

            return `
                <div class="provider-config-section" data-provider="${key}">
                    <div class="provider-config-header">${p.name}${mainRadio}</div>
                    ${keyField}
                    <div class="settings-field">
                        <label>Model</label>
                        <div class="model-select-wrap" data-provider-model-wrap="${key}">
                            <select class="provider-model-select" data-provider="${key}">
                                <option value="">Select a model...</option>
                            </select>
                        </div>
                    </div>
                </div>
            `;
        }).join('');

        // Embedding warning: check if any selected provider supports embeddings
        const hasEmbeddingProvider = this.selectedProviders.some(
            p => this.embeddingProviders.includes(p)
        );
        const embeddingWarning = !hasEmbeddingProvider && this.selectedProviders.length > 0 ? `
            <div class="provider-warning">
                <span class="provider-warning-icon">&#9888;</span>
                <span>None of the selected providers offer an embedding API.
                Memory search requires embeddings — add OpenAI, Gemini, or Ollama to enable it.</span>
            </div>
        ` : '';

        return `
            <h2>Add Providers</h2>
            <p class="subtitle">Select one or more LLM providers. You can mix providers across roles.</p>
            <div id="provider-list">${options}</div>
            ${embeddingWarning}
            ${configSections ? `<div id="provider-configs">${configSections}</div>` : ''}
            <div class="setup-nav">
                <button class="btn btn-secondary" id="setup-back">Back</button>
                <button class="btn btn-primary" id="setup-next">Next</button>
            </div>
        `;
    },

    // ── Step 2: Roles ─────────────────────────────────────────────────

    renderRoles() {
        const checked = this.useDefaultRoles ? 'checked' : '';
        const roleNames = { observer: 'Observer (memory extraction)', reflector: 'Reflector (memory compression)', pulse: 'Pulse (scheduled wake turns)' };

        const roleRows = Object.entries(roleNames).map(([key, label]) => {
            const r = this.roles[key];
            const prov = r.provider || this.mainProvider;
            // Only show API key field if the role uses a provider not in selectedProviders
            const needsKey = prov !== 'ollama' && !this.selectedProviders.includes(prov);

            return `
                <div class="role-row" data-role="${key}">
                    <div class="role-row-label">${label}</div>
                    <div class="role-row-fields">
                        <div class="settings-field">
                            <label>Provider</label>
                            <select data-role-field="provider" data-role="${key}">
                                ${Object.entries(this.providers).map(([pk, pp]) =>
                                    `<option value="${pk}" ${pk === prov ? 'selected' : ''}>${pp.name}</option>`
                                ).join('')}
                            </select>
                        </div>
                        <div class="settings-field">
                            <label>Model</label>
                            <div class="model-select-wrap">
                                <select data-role-field="model" data-role="${key}">
                                    <option value="">Loading...</option>
                                </select>
                            </div>
                        </div>
                    </div>
                    ${needsKey ? `
                        <div class="settings-field" style="margin-top:8px">
                            <label>API Key</label>
                            <input type="password" data-role-field="apiKey" data-role="${key}"
                                value="${esc(r.apiKey || '')}" placeholder="Key for ${this.providers[prov]?.name || prov}">
                        </div>
                    ` : ''}
                </div>
            `;
        }).join('');

        return `
            <h2>Model Roles</h2>
            <p class="subtitle">Assign models for memory and proactivity subsystems. Defaults to the same as your main model.</p>
            <div class="settings-field">
                <label>
                    <input type="checkbox" id="setup-use-defaults" ${checked}>
                    Use main model for all roles
                </label>
            </div>
            <div id="role-fields" style="${this.useDefaultRoles ? 'display:none' : ''}">
                ${roleRows}
            </div>
            <div class="setup-nav">
                <button class="btn btn-secondary" id="setup-back">Back</button>
                <button class="btn btn-primary" id="setup-next">Next</button>
            </div>
        `;
    },

    // ── Step 3: MCP Servers ───────────────────────────────────────────

    renderMcp() {
        const items = this.catalog.map((srv, i) => {
            const added = this.mcpServers.some(s => s.name === srv.name);
            return `
                <div class="mcp-item ${added ? 'added' : ''}" data-idx="${i}">
                    <div class="mcp-info">
                        <div class="mcp-name">${esc(srv.name)}</div>
                        <div class="mcp-desc">${esc(srv.description)}</div>
                    </div>
                    <button class="mcp-add-btn" data-idx="${i}">${added ? 'Added' : 'Add'}</button>
                </div>
            `;
        }).join('');

        return `
            <h2>MCP Servers</h2>
            <p class="subtitle">Optionally add tool servers. You can always add more later in settings.</p>
            ${items || '<p style="color:var(--text-dim)">No catalog entries available.</p>'}
            <div class="setup-nav">
                <button class="btn btn-secondary" id="setup-back">Back</button>
                <button class="btn btn-primary" id="setup-next">Next</button>
            </div>
        `;
    },

    // ── Step 4: Review ────────────────────────────────────────────────

    renderReview() {
        const toml = this.generateToml();
        return `
            <h2>Review Configuration</h2>
            <p class="subtitle">Here's your generated config. Edit if needed, then save to start IronClaw.</p>
            <textarea class="toml-editor" id="setup-toml">${esc(toml)}</textarea>
            <div class="validation-msg" id="setup-validation"></div>
            <div class="setup-nav">
                <button class="btn btn-secondary" id="setup-back">Back</button>
                <button class="btn btn-primary" id="setup-save">Save &amp; Start</button>
            </div>
        `;
    },

    // ── Step binding ──────────────────────────────────────────────────

    bindStep() {
        const next = document.getElementById('setup-next');
        const back = document.getElementById('setup-back');
        const save = document.getElementById('setup-save');

        if (next) next.addEventListener('click', () => this.nextStep());
        if (back) back.addEventListener('click', () => this.prevStep());
        if (save) save.addEventListener('click', () => this.saveConfig());

        // Step-specific bindings
        if (this.step === 1) {
            this.bindProviderStep();
        }

        if (this.step === 2) {
            this.bindRolesStep();
        }

        if (this.step === 3) {
            document.querySelectorAll('.mcp-add-btn').forEach(btn => {
                btn.addEventListener('click', () => {
                    const idx = parseInt(btn.dataset.idx, 10);
                    const srv = this.catalog[idx];
                    if (!srv) return;

                    const exists = this.mcpServers.findIndex(s => s.name === srv.name);
                    if (exists >= 0) {
                        this.mcpServers.splice(exists, 1);
                    } else {
                        // Check if server needs input
                        if (srv.requires_input && srv.requires_input.length > 0) {
                            for (const req of srv.requires_input) {
                                const val = prompt(`${req.label}:`);
                                if (val === null) return; // cancelled
                                this.setNestedField(srv, req.field, val);
                            }
                        }
                        this.mcpServers.push({
                            name: srv.name,
                            command: srv.command,
                            args: srv.args || [],
                            env: srv.env || {}
                        });
                    }
                    this.render();
                });
            });
        }
    },

    bindProviderStep() {
        // Provider toggle (multi-select)
        document.querySelectorAll('.provider-option').forEach(el => {
            el.addEventListener('click', () => {
                const key = el.dataset.provider;
                const idx = this.selectedProviders.indexOf(key);
                if (idx >= 0) {
                    // Don't allow deselecting the last provider
                    if (this.selectedProviders.length <= 1) return;
                    this.selectedProviders.splice(idx, 1);
                    // If we removed the main provider, pick the first remaining
                    if (this.mainProvider === key) {
                        this.mainProvider = this.selectedProviders[0];
                    }
                } else {
                    this.selectedProviders.push(key);
                }
                this.render();
            });
        });

        // Main provider radio buttons
        document.querySelectorAll('input[name="main-provider"]').forEach(radio => {
            radio.addEventListener('change', () => {
                this.mainProvider = radio.value;
            });
        });

        // Per-provider API key inputs (debounced)
        document.querySelectorAll('.provider-apikey').forEach(inp => {
            const prov = inp.dataset.provider;
            const debouncedFetch = ModelFetcher.debounce(() => {
                this.providerConfigs[prov].apiKey = inp.value;
                this.fetchProviderModels(prov);
            }, 500);
            inp.addEventListener('input', debouncedFetch);
        });

        // Fetch models for all selected providers on render
        for (const prov of this.selectedProviders) {
            this.fetchProviderModels(prov);
        }
    },

    async fetchProviderModels(provider) {
        const selectEl = document.querySelector(`.provider-model-select[data-provider="${provider}"]`);
        const errorEl = document.querySelector(`[data-provider-error="${provider}"]`);
        const wrapEl = document.querySelector(`[data-provider-model-wrap="${provider}"]`);

        if (!selectEl) return;
        if (wrapEl) wrapEl.classList.add('loading');

        const cfg = this.providerConfigs[provider];
        const key = provider !== 'ollama' ? cfg.apiKey : null;
        await ModelFetcher.populateSelect(selectEl, provider, key, cfg.url || null, cfg.model);

        if (wrapEl) wrapEl.classList.remove('loading');

        // Show API key error if applicable
        if (errorEl) {
            const cacheKey = ModelFetcher._cacheKey(provider, key, cfg.url || null);
            const cached = ModelFetcher.cache[cacheKey];
            if (!cached && key) {
                const result = await ModelFetcher.fetch(provider, key, cfg.url || null);
                if (result.error) {
                    errorEl.textContent = result.error;
                    errorEl.style.display = 'block';
                } else {
                    errorEl.style.display = 'none';
                }
            } else {
                errorEl.style.display = 'none';
            }
        }
    },

    bindRolesStep() {
        const cb = document.getElementById('setup-use-defaults');
        if (cb) {
            cb.addEventListener('change', () => {
                this.useDefaultRoles = cb.checked;
                const fields = document.getElementById('role-fields');
                if (fields) {
                    fields.style.display = cb.checked ? 'none' : '';
                }
            });
        }

        // Provider change per role — refetch models
        document.querySelectorAll('[data-role-field="provider"]').forEach(sel => {
            sel.addEventListener('change', () => {
                const role = sel.dataset.role;
                this.roles[role].provider = sel.value;
                this.roles[role].model = '';
                this.roles[role].apiKey = '';
                // Re-render to show/hide API key field
                this.collectRoleFields();
                this.render();
            });
        });

        // API key debounced input per role
        document.querySelectorAll('[data-role-field="apiKey"]').forEach(inp => {
            const role = inp.dataset.role;
            const debouncedFetch = ModelFetcher.debounce(() => {
                this.roles[role].apiKey = inp.value;
                this.fetchRoleModels(role);
            }, 500);
            inp.addEventListener('input', debouncedFetch);
        });

        // Populate model dropdowns for each role
        if (!this.useDefaultRoles) {
            for (const role of ['observer', 'reflector', 'pulse']) {
                this.fetchRoleModels(role);
            }
        }
    },

    async fetchRoleModels(role) {
        const selectEl = document.querySelector(`[data-role-field="model"][data-role="${role}"]`);
        if (!selectEl) return;

        const r = this.roles[role];
        const prov = r.provider || this.mainProvider;
        // Use provider config key if we have it, otherwise role's own key
        let key = null;
        if (prov !== 'ollama') {
            const provCfg = this.providerConfigs[prov];
            key = provCfg ? provCfg.apiKey : r.apiKey;
        }

        await ModelFetcher.populateSelect(selectEl, prov, key, r.url || null, r.model);
    },

    collectRoleFields() {
        for (const role of ['observer', 'reflector', 'pulse']) {
            const provSel = document.querySelector(`[data-role-field="provider"][data-role="${role}"]`);
            const modelSel = document.querySelector(`[data-role-field="model"][data-role="${role}"]`);
            const keyInp = document.querySelector(`[data-role-field="apiKey"][data-role="${role}"]`);

            if (provSel) this.roles[role].provider = provSel.value;
            if (modelSel) this.roles[role].model = ModelFetcher.getSelectedModel(modelSel);
            if (keyInp) this.roles[role].apiKey = keyInp.value;
        }
    },

    setNestedField(obj, path, value) {
        const parts = path.split('.');
        let target = obj;
        for (let i = 0; i < parts.length - 1; i++) {
            if (!target[parts[i]]) target[parts[i]] = {};
            target = target[parts[i]];
        }
        target[parts[parts.length - 1]] = value;
    },

    collectCurrentStep() {
        switch (this.step) {
            case 0:
                this.timezone = (document.getElementById('setup-tz') || {}).value || this.timezone;
                break;
            case 1: {
                // Collect per-provider fields
                for (const prov of this.selectedProviders) {
                    const keyEl = document.querySelector(`.provider-apikey[data-provider="${prov}"]`);
                    if (keyEl) this.providerConfigs[prov].apiKey = keyEl.value;
                    const modelEl = document.querySelector(`.provider-model-select[data-provider="${prov}"]`);
                    if (modelEl) this.providerConfigs[prov].model = ModelFetcher.getSelectedModel(modelEl);
                }
                // Collect main provider radio
                const mainRadio = document.querySelector('input[name="main-provider"]:checked');
                if (mainRadio) this.mainProvider = mainRadio.value;
                break;
            }
            case 2:
                this.useDefaultRoles = (document.getElementById('setup-use-defaults') || {}).checked !== false;
                if (!this.useDefaultRoles) {
                    this.collectRoleFields();
                }
                break;
        }
    },

    nextStep() {
        this.collectCurrentStep();
        if (this.step < this.totalSteps - 1) {
            this.step++;
            this.render();
        }
    },

    prevStep() {
        this.collectCurrentStep();
        if (this.step > 0) {
            this.step--;
            this.render();
        }
    },

    // ── TOML generation ───────────────────────────────────────────────

    generateToml() {
        const lines = [];
        lines.push(`timezone = "${this.timezone}"`);
        lines.push('');

        // Collect which providers we need entries for
        const providerEntries = {};

        // All selected providers
        for (const prov of this.selectedProviders) {
            const cfg = this.providerConfigs[prov];
            if (prov !== 'ollama' && cfg.apiKey) {
                providerEntries[prov] = {
                    type: prov,
                    api_key: cfg.apiKey,
                    url: cfg.url || null,
                };
            }
        }

        // Role providers (if different from selected ones)
        if (!this.useDefaultRoles) {
            for (const role of ['observer', 'reflector', 'pulse']) {
                const r = this.roles[role];
                const prov = r.provider || this.mainProvider;
                if (!providerEntries[prov] && prov !== 'ollama' && r.apiKey) {
                    providerEntries[prov] = {
                        type: prov,
                        api_key: r.apiKey,
                    };
                }
            }
        }

        // Write provider entries
        for (const [name, cfg] of Object.entries(providerEntries)) {
            lines.push(`[providers.${name}]`);
            lines.push(`type = "${cfg.type}"`);
            lines.push(`api_key = "${cfg.api_key}"`);
            if (cfg.url) lines.push(`url = "${cfg.url}"`);
            lines.push('');
        }

        const mainCfg = this.providerConfigs[this.mainProvider];
        const mainModel = mainCfg.model || ModelFetcher.defaultModels[this.mainProvider] || '';
        lines.push('[models]');
        lines.push(`main = "${this.mainProvider}/${mainModel}"`);

        if (!this.useDefaultRoles) {
            for (const role of ['observer', 'reflector', 'pulse']) {
                const r = this.roles[role];
                const prov = r.provider || this.mainProvider;
                const model = r.model;
                if (model) {
                    lines.push(`${role} = "${prov}/${model}"`);
                }
            }
        }

        // MCP servers
        if (this.mcpServers.length > 0) {
            lines.push('');
            for (const srv of this.mcpServers) {
                lines.push(`[mcp.servers.${srv.name}]`);
                lines.push(`command = "${srv.command}"`);
                if (srv.args && srv.args.length > 0) {
                    const argsStr = srv.args.map(a => `"${a}"`).join(', ');
                    lines.push(`args = [${argsStr}]`);
                }
                if (srv.env && Object.keys(srv.env).length > 0) {
                    const envParts = Object.entries(srv.env)
                        .map(([k, v]) => `${k} = "${v}"`).join(', ');
                    lines.push(`env = { ${envParts} }`);
                }
                lines.push('');
            }
        }

        return lines.join('\n');
    },

    async saveConfig() {
        const toml = (document.getElementById('setup-toml') || {}).value || this.generateToml();
        const validationEl = document.getElementById('setup-validation');
        const saveBtn = document.getElementById('setup-save');

        if (saveBtn) saveBtn.disabled = true;

        try {
            const res = await fetch('/api/config/complete-setup', {
                method: 'POST',
                headers: { 'Content-Type': 'text/plain' },
                body: toml
            });
            const data = await res.json();

            if (data.valid) {
                if (validationEl) {
                    validationEl.className = 'validation-msg success';
                    validationEl.textContent = 'Configuration saved! Starting gateway...';
                }
                // Give the server a moment to restart, then switch to chat
                setTimeout(() => App.onSetupComplete(), 1500);
            } else {
                if (validationEl) {
                    validationEl.className = 'validation-msg error';
                    validationEl.textContent = data.error || 'Validation failed';
                }
                if (saveBtn) saveBtn.disabled = false;
            }
        } catch (err) {
            if (validationEl) {
                validationEl.className = 'validation-msg error';
                validationEl.textContent = 'Network error: ' + err.message;
            }
            if (saveBtn) saveBtn.disabled = false;
        }
    }
};

function esc(text) {
    const d = document.createElement('div');
    d.textContent = text;
    return d.innerHTML;
}
