// ── IronClaw Web UI — Setup Wizard ────────────────────────────────────
//
// Multi-step wizard for first-run configuration. Generates a valid
// config.toml and POSTs it to /api/config/complete-setup.

const Setup = {
    step: 0,
    totalSteps: 5,

    // Collected state
    userName: '',
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
    useDefaultRoles: false,
    // Per-role state: { provider, apiKey, url, model }
    roles: {
        observer: { provider: '', apiKey: '', url: '', model: '' },
        reflector: { provider: '', apiKey: '', url: '', model: '' },
        pulse: { provider: '', apiKey: '', url: '', model: '' }
    },
    // Extra model slots
    embeddingModel: { provider: '', model: '' },
    backgroundModels: {
        small: { provider: '', model: '' },
        medium: { provider: '', model: '' },
        large: { provider: '', model: '' }
    },

    // Providers that support embedding models
    embeddingProviders: ['openai', 'gemini', 'ollama'],
    mcpServers: [],  // [{name, command, args, env}]
    mcpPendingIdx: null,  // catalog index currently showing inline input fields
    catalog: [],

    providers: {
        anthropic: {
            name: 'Anthropic',
            desc: 'Claude models (Sonnet, Haiku, Opus)',
            keyEnv: 'ANTHROPIC_API_KEY'
        },
        openai: {
            name: 'OpenAI',
            desc: 'OpenAI or compatible APIs (vLLM, LM Studio, etc.)',
            keyEnv: 'OPENAI_API_KEY',
            showUrl: true
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
                <label>Your Name</label>
                <input type="text" id="setup-name" value="${esc(this.userName)}"
                    placeholder="What should your agent call you?">
            </div>
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

            const urlField = p.showUrl ? `
                <div class="settings-field">
                    <label>Base URL (leave blank for default)</label>
                    <input type="text" class="provider-url" data-provider="${key}"
                        value="${esc(cfg.url)}" placeholder="https://api.openai.com/v1">
                </div>
            ` : '';

            return `
                <div class="provider-config-section" data-provider="${key}">
                    <div class="provider-config-header">${p.name}</div>
                    ${keyField}
                    ${urlField}
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
                Memory search works best with embeddings — consider adding OpenAI, Gemini, or Ollama.</span>
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
                <span class="setup-nav-hint">You can add more providers later in settings.</span>
                <button class="btn btn-primary" id="setup-next">Next</button>
            </div>
        `;
    },

    // ── Step 2: Models ────────────────────────────────────────────────

    renderRoles() {
        // Build a provider/model row for a given role
        const roleRow = (roleKey, label, data) => {
            const prov = data.provider || this.mainProvider;
            const needsKey = prov !== 'ollama' && !this.selectedProviders.includes(prov);

            return `
                <div class="role-row" data-role="${roleKey}">
                    <div class="role-row-label">${label}</div>
                    <div class="role-row-fields">
                        <div class="settings-field">
                            <label>Provider</label>
                            <select data-role-field="provider" data-role="${roleKey}">
                                ${Object.entries(this.providers).map(([pk, pp]) =>
                                    `<option value="${pk}" ${pk === prov ? 'selected' : ''}>${pp.name}</option>`
                                ).join('')}
                            </select>
                        </div>
                        <div class="settings-field">
                            <label>Model</label>
                            <div class="model-select-wrap">
                                <select data-role-field="model" data-role="${roleKey}">
                                    <option value="">Loading...</option>
                                </select>
                            </div>
                        </div>
                    </div>
                    ${needsKey ? `
                        <div class="settings-field" style="margin-top:8px">
                            <label>API Key</label>
                            <input type="password" data-role-field="apiKey" data-role="${roleKey}"
                                value="${esc(data.apiKey || '')}" placeholder="Key for ${this.providers[prov]?.name || prov}">
                        </div>
                    ` : ''}
                </div>
            `;
        };

        // Main model — provider selectable from configured providers
        const mainProv = this.mainProvider;
        const mainRow = `
            <div class="role-row" data-role="main">
                <div class="role-row-label">Main Agent</div>
                <div class="role-row-fields">
                    <div class="settings-field">
                        <label>Provider</label>
                        <select data-role-field="provider" data-role="main">
                            ${this.selectedProviders.map(pk =>
                                `<option value="${pk}" ${pk === mainProv ? 'selected' : ''}>${this.providers[pk].name}</option>`
                            ).join('')}
                        </select>
                    </div>
                    <div class="settings-field">
                        <label>Model</label>
                        <div class="model-select-wrap">
                            <select data-role-field="model" data-role="main">
                                <option value="">Loading...</option>
                            </select>
                        </div>
                    </div>
                </div>
            </div>
        `;

        // Subsystem roles
        const roleNames = {
            observer: 'Observer',
            reflector: 'Reflector',
            pulse: 'Pulse'
        };
        const subsystemRows = Object.entries(roleNames).map(([key, label]) =>
            roleRow(key, label, this.roles[key])
        ).join('');

        // Embedding model — only show providers that support it
        const embProv = this.embeddingModel.provider || this._defaultEmbeddingProvider();
        const embeddingRow = embProv ? `
            <div class="role-row" data-role="embedding">
                <div class="role-row-label">Embedding</div>
                <div class="role-row-fields">
                    <div class="settings-field">
                        <label>Provider</label>
                        <select data-role-field="provider" data-role="embedding">
                            ${this.embeddingProviders.map(pk => {
                                const pp = this.providers[pk];
                                return `<option value="${pk}" ${pk === embProv ? 'selected' : ''}>${pp.name}</option>`;
                            }).join('')}
                        </select>
                    </div>
                    <div class="settings-field">
                        <label>Model</label>
                        <div class="model-select-wrap">
                            <select data-role-field="model" data-role="embedding">
                                <option value="">Loading...</option>
                            </select>
                        </div>
                    </div>
                </div>
            </div>
        ` : '';

        // Background model tiers
        const bgTiers = { small: 'Small', medium: 'Medium', large: 'Large' };
        const bgRows = Object.entries(bgTiers).map(([key, label]) => {
            const bg = this.backgroundModels[key];
            return roleRow(`bg-${key}`, label, bg);
        }).join('');

        return `
            <h2>Assign Models</h2>
            <p class="subtitle">Choose which model to use for each role. All default to the main model if left unchanged.</p>

            <div class="roles-section">
                <div class="roles-section-label">Agent</div>
                ${mainRow}
            </div>

            <div class="roles-section">
                <div class="roles-section-label">Subsystems</div>
                <p class="roles-section-hint">Memory and proactivity subsystems. These can use smaller, cheaper models.</p>
                ${subsystemRows}
            </div>

            ${embeddingRow ? `
            <div class="roles-section">
                <div class="roles-section-label">Embedding</div>
                <p class="roles-section-hint">Used for semantic memory search. Anthropic does not offer embeddings.</p>
                ${embeddingRow}
            </div>
            ` : ''}

            <div class="roles-section">
                <div class="roles-section-label">Background Tasks</div>
                <p class="roles-section-hint">Tiered models for background work. Tasks specify small, medium, or large.</p>
                ${bgRows}
            </div>

            <div class="setup-nav">
                <button class="btn btn-secondary" id="setup-back">Back</button>
                <button class="btn btn-primary" id="setup-next">Next</button>
            </div>
        `;
    },

    /** Pick the first selected provider that supports embeddings, or null. */
    _defaultEmbeddingProvider() {
        return this.selectedProviders.find(p => this.embeddingProviders.includes(p))
            || this.embeddingProviders[0] || null;
    },

    // ── Step 3: MCP Servers ───────────────────────────────────────────

    renderMcp() {
        const items = this.catalog.map((srv, i) => {
            const added = this.mcpServers.some(s => s.name === srv.name);
            const isPending = this.mcpPendingIdx === i;
            const needsInput = srv.requires_input && srv.requires_input.length > 0;

            let inputFields = '';
            if (isPending && needsInput) {
                const fields = srv.requires_input.map(req => `
                    <div class="settings-field mcp-input-field">
                        <label>${esc(req.label)}</label>
                        <input type="text" class="mcp-required-input"
                            data-field="${esc(req.field)}" data-idx="${i}"
                            placeholder="${esc(req.label)}">
                    </div>
                `).join('');
                inputFields = `
                    <div class="mcp-inline-inputs">
                        ${fields}
                        <div class="mcp-inline-actions">
                            <button class="btn btn-primary btn-sm mcp-confirm-btn" data-idx="${i}">Add</button>
                            <button class="btn btn-secondary btn-sm mcp-cancel-btn" data-idx="${i}">Cancel</button>
                        </div>
                    </div>
                `;
            }

            const btnLabel = added ? 'Added' : (isPending ? '' : 'Add');
            const btnHidden = isPending ? 'style="display:none"' : '';

            return `
                <div class="mcp-item ${added ? 'added' : ''} ${isPending ? 'pending' : ''}" data-idx="${i}">
                    <div class="mcp-info">
                        <div class="mcp-name">${esc(srv.name)}</div>
                        <div class="mcp-desc">${esc(srv.description)}</div>
                    </div>
                    <button class="mcp-add-btn" data-idx="${i}" ${btnHidden}>${btnLabel}</button>
                    ${inputFields}
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
            this.bindMcpStep();
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

        // Per-provider API key inputs
        document.querySelectorAll('.provider-apikey').forEach(inp => {
            const prov = inp.dataset.provider;
            inp.addEventListener('input', () => {
                this.providerConfigs[prov].apiKey = inp.value;
            });
        });

        // Per-provider URL inputs
        document.querySelectorAll('.provider-url').forEach(inp => {
            const prov = inp.dataset.provider;
            inp.addEventListener('input', () => {
                this.providerConfigs[prov].url = inp.value;
            });
        });
    },

    bindMcpStep() {
        // Add/remove buttons (for items without required input, or toggling off)
        document.querySelectorAll('.mcp-add-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                const idx = parseInt(btn.dataset.idx, 10);
                const srv = this.catalog[idx];
                if (!srv) return;

                const exists = this.mcpServers.findIndex(s => s.name === srv.name);
                if (exists >= 0) {
                    // Toggle off — remove
                    this.mcpServers.splice(exists, 1);
                    this.mcpPendingIdx = null;
                    this.render();
                } else if (srv.requires_input && srv.requires_input.length > 0) {
                    // Show inline input fields
                    this.mcpPendingIdx = idx;
                    this.render();
                    // Focus the first input
                    const first = document.querySelector(`.mcp-required-input[data-idx="${idx}"]`);
                    if (first) first.focus();
                } else {
                    // No input needed — add directly
                    this.mcpServers.push({
                        name: srv.name,
                        command: srv.command,
                        args: srv.args || [],
                        env: srv.env || {}
                    });
                    this.render();
                }
            });
        });

        // Confirm button — collect inline inputs and add the server
        document.querySelectorAll('.mcp-confirm-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                const idx = parseInt(btn.dataset.idx, 10);
                const srv = this.catalog[idx];
                if (!srv) return;

                // Collect values from inline inputs
                const inputs = document.querySelectorAll(`.mcp-required-input[data-idx="${idx}"]`);
                for (const inp of inputs) {
                    const val = inp.value.trim();
                    if (!val) {
                        inp.focus();
                        inp.classList.add('input-error');
                        return;
                    }
                    this.setNestedField(srv, inp.dataset.field, val);
                }

                this.mcpServers.push({
                    name: srv.name,
                    command: srv.command,
                    args: srv.args || [],
                    env: srv.env || {}
                });
                this.mcpPendingIdx = null;
                this.render();
            });
        });

        // Cancel button — collapse the inline inputs
        document.querySelectorAll('.mcp-cancel-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                this.mcpPendingIdx = null;
                this.render();
            });
        });
    },

    bindRolesStep() {
        // Provider change per role — clear model and re-render
        // Don't collectRoleFields here: the DOM still has stale values
        document.querySelectorAll('[data-role-field="provider"]').forEach(sel => {
            sel.addEventListener('change', () => {
                const role = sel.dataset.role;
                this._setRoleData(role, 'provider', sel.value);
                this._setRoleData(role, 'model', '');
                const data = this._getRoleData(role);
                if (data && data.apiKey !== undefined) {
                    data.apiKey = '';
                }
                this.render();
            });
        });

        // API key debounced input per role
        document.querySelectorAll('[data-role-field="apiKey"]').forEach(inp => {
            const role = inp.dataset.role;
            const debouncedFetch = ModelFetcher.debounce(() => {
                this._setRoleData(role, 'apiKey', inp.value);
                this.fetchRoleModels(role);
            }, 500);
            inp.addEventListener('input', debouncedFetch);
        });

        // Populate model dropdowns for all visible roles
        const allRoles = ['main', 'observer', 'reflector', 'pulse', 'embedding', 'bg-small', 'bg-medium', 'bg-large'];
        for (const role of allRoles) {
            this.fetchRoleModels(role);
        }
    },

    /** Get the data object for a role key (handles main, embedding, bg-* and subsystem roles). */
    _getRoleData(role) {
        if (role === 'main') return this.providerConfigs[this.mainProvider];
        if (role === 'embedding') return this.embeddingModel;
        if (role.startsWith('bg-')) return this.backgroundModels[role.slice(3)];
        return this.roles[role];
    },

    /** Set a field on the data object for a role key. */
    _setRoleData(role, field, value) {
        if (role === 'main' && field === 'provider') {
            this.mainProvider = value;
            // Clear model on the new provider so it picks up the default
            this.providerConfigs[value].model = '';
            return;
        }
        const data = this._getRoleData(role);
        if (data) data[field] = value;
    },

    /** Get the provider key for a role (resolving defaults). */
    _getRoleProvider(role) {
        if (role === 'main') return this.mainProvider;
        if (role === 'embedding') return this.embeddingModel.provider || this._defaultEmbeddingProvider();
        const data = this._getRoleData(role);
        return (data && data.provider) || this.mainProvider;
    },

    async fetchRoleModels(role) {
        const selectEl = document.querySelector(`[data-role-field="model"][data-role="${role}"]`);
        if (!selectEl) return;

        const prov = this._getRoleProvider(role);
        const data = this._getRoleData(role);
        // Use provider config key if we have it, otherwise role's own key
        let key = null;
        if (prov !== 'ollama') {
            const provCfg = this.providerConfigs[prov];
            key = provCfg ? provCfg.apiKey : (data.apiKey || '');
        }
        const url = (this.providerConfigs[prov] && this.providerConfigs[prov].url) || '';

        // For embedding role, use embedding-specific default
        const fallback = role === 'embedding'
            ? (ModelFetcher.defaultEmbeddingModels[prov] || '')
            : (ModelFetcher.defaultModels[prov] || '');

        await ModelFetcher.populateSelect(selectEl, prov, key, url || null, data.model || '', fallback);
    },

    collectRoleFields() {
        // Subsystem roles
        for (const role of ['observer', 'reflector', 'pulse']) {
            const provSel = document.querySelector(`[data-role-field="provider"][data-role="${role}"]`);
            const modelSel = document.querySelector(`[data-role-field="model"][data-role="${role}"]`);
            const keyInp = document.querySelector(`[data-role-field="apiKey"][data-role="${role}"]`);

            if (provSel) this.roles[role].provider = provSel.value;
            if (modelSel) this.roles[role].model = ModelFetcher.getSelectedModel(modelSel);
            if (keyInp) this.roles[role].apiKey = keyInp.value;
        }

        // Main model
        const mainModelSel = document.querySelector(`[data-role-field="model"][data-role="main"]`);
        if (mainModelSel) this.providerConfigs[this.mainProvider].model = ModelFetcher.getSelectedModel(mainModelSel);

        // Embedding
        const embProvSel = document.querySelector(`[data-role-field="provider"][data-role="embedding"]`);
        const embModelSel = document.querySelector(`[data-role-field="model"][data-role="embedding"]`);
        if (embProvSel) this.embeddingModel.provider = embProvSel.value;
        if (embModelSel) this.embeddingModel.model = ModelFetcher.getSelectedModel(embModelSel);

        // Background tiers
        for (const tier of ['small', 'medium', 'large']) {
            const provSel = document.querySelector(`[data-role-field="provider"][data-role="bg-${tier}"]`);
            const modelSel = document.querySelector(`[data-role-field="model"][data-role="bg-${tier}"]`);
            if (provSel) this.backgroundModels[tier].provider = provSel.value;
            if (modelSel) this.backgroundModels[tier].model = ModelFetcher.getSelectedModel(modelSel);
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
                this.userName = (document.getElementById('setup-name') || {}).value || this.userName;
                this.timezone = (document.getElementById('setup-tz') || {}).value || this.timezone;
                break;
            case 1: {
                // Collect per-provider fields
                for (const prov of this.selectedProviders) {
                    const keyEl = document.querySelector(`.provider-apikey[data-provider="${prov}"]`);
                    if (keyEl) this.providerConfigs[prov].apiKey = keyEl.value;
                    const urlEl = document.querySelector(`.provider-url[data-provider="${prov}"]`);
                    if (urlEl) this.providerConfigs[prov].url = urlEl.value;
                }
                break;
            }
            case 2:
                this.collectRoleFields();
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
        if (this.userName) lines.push(`name = "${this.userName}"`);
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

        // Write provider entries
        for (const [name, cfg] of Object.entries(providerEntries)) {
            lines.push(`[providers.${name}]`);
            lines.push(`type = "${cfg.type}"`);
            lines.push(`api_key = "${cfg.api_key}"`);
            if (cfg.url) lines.push(`url = "${cfg.url}"`);
            lines.push('');
        }

        // Models section
        const mainCfg = this.providerConfigs[this.mainProvider];
        const mainModel = mainCfg.model || ModelFetcher.defaultModels[this.mainProvider] || '';
        lines.push('[models]');
        lines.push(`main = "${this.mainProvider}/${mainModel}"`);

        for (const role of ['observer', 'reflector', 'pulse']) {
            const r = this.roles[role];
            const prov = r.provider || this.mainProvider;
            const model = r.model;
            if (model) {
                lines.push(`${role} = "${prov}/${model}"`);
            }
        }

        // Embedding model
        const embProv = this.embeddingModel.provider || this._defaultEmbeddingProvider();
        if (embProv && this.embeddingModel.model) {
            lines.push(`embedding = "${embProv}/${this.embeddingModel.model}"`);
        }

        // Background models
        const bgEntries = [];
        for (const tier of ['small', 'medium', 'large']) {
            const bg = this.backgroundModels[tier];
            const prov = bg.provider || this.mainProvider;
            if (bg.model) {
                bgEntries.push({ tier, prov, model: bg.model });
            }
        }
        if (bgEntries.length > 0) {
            lines.push('');
            lines.push('[background.models]');
            for (const { tier, prov, model } of bgEntries) {
                lines.push(`${tier} = "${prov}/${model}"`);
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
