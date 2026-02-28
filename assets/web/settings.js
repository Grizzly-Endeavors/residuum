// ── IronClaw Web UI — Settings Panel ──────────────────────────────────
//
// Form-based config editing with raw TOML advanced mode.
// Uses smol-toml (window.TOML) for client-side parse/stringify.
// Single source of truth: this.currentToml. Form fields read/write
// a configObj parsed from it, then stringify back on save.

const Settings = {
    initialized: false,
    currentToml: '',
    configObj: {},
    catalog: [],
    mode: 'form', // 'form' | 'advanced'

    // Section definitions — each maps to a TOML table
    sections: [
        { key: 'general', title: 'General', icon: '\u2699' },
        { key: 'providers', title: 'Providers', icon: '\u26A1' },
        { key: 'models', title: 'Models', icon: '\u2B22' },
        { key: 'memory', title: 'Memory', icon: '\u{1F9E0}' },
        { key: 'pulse', title: 'Pulse', icon: '\u2764' },
        { key: 'gateway', title: 'Gateway', icon: '\u{1F310}' },
        { key: 'discord', title: 'Discord', icon: '\u{1F4AC}' },
        { key: 'webhook', title: 'Webhook', icon: '\u{1F517}' },
        { key: 'skills', title: 'Skills', icon: '\u{1F4C1}' },
        { key: 'retry', title: 'Retry', icon: '\u21BA' },
        { key: 'background', title: 'Background', icon: '\u23F3' },
        { key: 'notifications', title: 'Notifications', icon: '\u{1F514}' },
        { key: 'mcp', title: 'MCP Servers', icon: '\u{1F50C}' },
    ],

    // Which sections are expanded
    openSections: new Set(['general', 'models']),

    async init() {
        if (this.initialized) return;
        this.initialized = true;

        const inner = document.getElementById('settings-inner');
        inner.innerHTML = this.renderSkeleton();

        const [tomlRes, catalogRes] = await Promise.all([
            fetch('/api/config/raw').then(r => r.text()).catch(() => ''),
            fetch('/api/mcp-catalog').then(r => r.json()).catch(() => [])
        ]);

        this.currentToml = tomlRes;
        this.catalog = catalogRes;
        this.parseToml();

        inner.innerHTML = this.render();
        this.bind();
    },

    invalidate() {
        this.initialized = false;
    },

    parseToml() {
        try {
            this.configObj = window.TOML ? window.TOML.parse(this.currentToml) : {};
        } catch {
            this.configObj = {};
        }
    },

    syncTomlFromObj() {
        try {
            if (window.TOML) {
                this.currentToml = window.TOML.stringify(this.configObj);
            }
        } catch { /* keep current toml on stringify failure */ }
    },

    renderSkeleton() {
        return '<p style="color:var(--text-dim);padding:20px">Loading configuration...</p>';
    },

    // ── Main render ──────────────────────────────────────────────────

    render() {
        if (this.mode === 'advanced') {
            return this.renderAdvanced();
        }
        return this.renderForm();
    },

    renderForm() {
        const sections = this.sections.map(s => this.renderSection(s)).join('');
        return `
            ${sections}
            <div class="settings-actions">
                <button class="btn btn-primary" id="settings-save">Save</button>
                <button class="btn btn-secondary" id="settings-refresh">Refresh</button>
            </div>
            <div class="validation-msg" id="settings-validation"></div>
        `;
    },

    renderAdvanced() {
        const hasComments = /^\s*#/m.test(this.currentToml);
        return `
            <div class="settings-section">
                <p style="color:var(--text-muted);font-size:12px;margin-bottom:12px">
                    Edit the raw TOML configuration. Changes are validated before saving.
                </p>
                ${!hasComments ? '' : `
                    <div class="comment-warning">
                        \u26A0 Switching to Form mode will strip comments from the TOML.
                    </div>
                `}
                <textarea class="toml-editor" id="settings-toml">${escAttr(this.currentToml)}</textarea>
                <div class="settings-actions">
                    <button class="btn btn-primary" id="settings-save">Validate &amp; Save</button>
                    <button class="btn btn-secondary" id="settings-validate">Validate Only</button>
                    <button class="btn btn-secondary" id="settings-refresh">Refresh</button>
                </div>
                <div class="validation-msg" id="settings-validation"></div>
            </div>
        `;
    },

    // ── Section renderer ─────────────────────────────────────────────

    renderSection(section) {
        const isOpen = this.openSections.has(section.key);
        const openCls = isOpen ? ' open' : '';
        const body = this.renderSectionBody(section.key);
        const badge = this.getSectionBadge(section.key);

        return `
            <div class="config-section${openCls}" data-section="${section.key}">
                <div class="config-section-header" data-section="${section.key}">
                    <span class="config-section-chevron">&#9654;</span>
                    <span class="config-section-title">${section.title}</span>
                    ${badge ? `<span class="config-section-badge">${badge}</span>` : ''}
                </div>
                <div class="config-section-body">
                    ${body}
                </div>
            </div>
        `;
    },

    getSectionBadge(key) {
        const obj = this.configObj;
        switch (key) {
            case 'providers': {
                const p = obj.providers || {};
                const count = Object.keys(p).length;
                return count > 0 ? `${count} configured` : '';
            }
            case 'models':
                return obj.models?.main || '';
            case 'pulse':
                return obj.pulse?.enabled === false ? 'disabled' : obj.pulse ? 'enabled' : '';
            case 'gateway':
                return obj.gateway ? `${obj.gateway.bind || '127.0.0.1'}:${obj.gateway.port || 7700}` : '';
            case 'discord':
                return obj.discord?.token ? 'configured' : '';
            case 'mcp': {
                const servers = obj.mcp?.servers || {};
                const count = Object.keys(servers).length;
                return count > 0 ? `${count} servers` : '';
            }
            case 'notifications': {
                const channels = obj.notifications?.channels || {};
                const count = Object.keys(channels).length;
                return count > 0 ? `${count} channels` : '';
            }
            default:
                return '';
        }
    },

    // ── Section body rendering ───────────────────────────────────────

    renderSectionBody(key) {
        const obj = this.configObj;
        switch (key) {
            case 'general':
                return this.renderGeneral(obj);
            case 'providers':
                return this.renderProviders(obj.providers || {});
            case 'models':
                return this.renderModels(obj.models || {});
            case 'memory':
                return this.renderMemory(obj.memory || {});
            case 'pulse':
                return this.renderPulse(obj.pulse || {});
            case 'gateway':
                return this.renderGateway(obj.gateway || {});
            case 'discord':
                return this.renderDiscord(obj.discord || {});
            case 'webhook':
                return this.renderWebhook(obj.webhook || {});
            case 'skills':
                return this.renderSkills(obj.skills || {});
            case 'retry':
                return this.renderRetry(obj.retry || {});
            case 'background':
                return this.renderBackground(obj.background || {});
            case 'notifications':
                return this.renderNotifications(obj.notifications || {});
            case 'mcp':
                return this.renderMcp(obj.mcp || {});
            default:
                return '';
        }
    },

    renderGeneral(obj) {
        return `
            <div class="settings-field">
                <label>Timezone (IANA format)</label>
                <input type="text" data-path="timezone" value="${escAttr(obj.timezone || '')}"
                    placeholder="America/New_York">
            </div>
            <div class="field-row">
                <div class="settings-field">
                    <label>Timeout (seconds)</label>
                    <input type="number" data-path="timeout_secs" value="${obj.timeout_secs ?? ''}"
                        placeholder="120">
                </div>
                <div class="settings-field">
                    <label>Max tokens</label>
                    <input type="number" data-path="max_tokens" value="${obj.max_tokens ?? ''}"
                        placeholder="8192">
                </div>
            </div>
            <div class="settings-field">
                <label>Workspace directory</label>
                <input type="text" data-path="workspace_dir" value="${escAttr(obj.workspace_dir || '')}"
                    placeholder="~/.ironclaw/workspace">
            </div>
        `;
    },

    renderProviders(providers) {
        const entries = Object.entries(providers);
        const rows = entries.map(([name, cfg]) => this.renderProviderRow(name, cfg)).join('');
        return `
            <div id="provider-entries">${rows}</div>
            <button class="add-entry-btn" data-add="provider">+ Add provider</button>
        `;
    },

    renderProviderRow(name, cfg) {
        return `
            <div class="entry-row" data-provider-name="${escAttr(name)}">
                <div class="entry-row-fields">
                    <div class="settings-field">
                        <label>Name</label>
                        <input type="text" data-provider-field="name" value="${escAttr(name)}">
                    </div>
                    <div class="settings-field">
                        <label>Type</label>
                        <select data-provider-field="type">
                            <option value="anthropic" ${cfg.type === 'anthropic' ? 'selected' : ''}>Anthropic</option>
                            <option value="openai" ${cfg.type === 'openai' ? 'selected' : ''}>OpenAI</option>
                            <option value="gemini" ${cfg.type === 'gemini' ? 'selected' : ''}>Gemini</option>
                            <option value="ollama" ${cfg.type === 'ollama' ? 'selected' : ''}>Ollama</option>
                        </select>
                    </div>
                    <div class="settings-field">
                        <label>API Key</label>
                        <input type="password" data-provider-field="api_key" value="${escAttr(cfg.api_key || '')}">
                    </div>
                    <div class="settings-field">
                        <label>URL (optional)</label>
                        <input type="text" data-provider-field="url" value="${escAttr(cfg.url || '')}"
                            placeholder="Default for provider type">
                    </div>
                </div>
                <button class="entry-remove-btn" data-remove-provider="${escAttr(name)}">&times;</button>
            </div>
        `;
    },

    /** Parse a "provider/model" string into { provider, model }. */
    parseModelSpec(spec) {
        if (!spec) return { provider: '', model: '' };
        const idx = spec.indexOf('/');
        if (idx < 0) return { provider: '', model: spec };
        return { provider: spec.substring(0, idx), model: spec.substring(idx + 1) };
    },

    /** Build a "provider/model" string from a provider and model. */
    buildModelSpec(provider, model) {
        if (!provider || !model) return '';
        return `${provider}/${model}`;
    },

    /** Get the available provider types from the config. */
    getConfiguredProviders() {
        const providers = this.configObj.providers || {};
        const types = new Set();
        for (const cfg of Object.values(providers)) {
            if (cfg.type) types.add(cfg.type);
        }
        // Always include all four types
        types.add('anthropic');
        types.add('openai');
        types.add('gemini');
        types.add('ollama');
        return [...types].sort();
    },

    /** Find API key and URL for a provider type from config. */
    getProviderCredentials(providerType) {
        const providers = this.configObj.providers || {};
        for (const cfg of Object.values(providers)) {
            if (cfg.type === providerType) {
                return { apiKey: cfg.api_key || '', url: cfg.url || '' };
            }
        }
        return { apiKey: '', url: '' };
    },

    renderModels(models) {
        const providerTypes = this.getConfiguredProviders();
        const roles = [
            { key: 'main', label: 'Main model (required)', required: true },
            { key: 'default', label: 'Default (fallback for unset roles)' },
            { key: 'observer', label: 'Observer' },
            { key: 'reflector', label: 'Reflector' },
            { key: 'pulse', label: 'Pulse' },
        ];

        const roleRows = roles.map(role => {
            const spec = this.parseModelSpec(models[role.key] || '');
            const provOptions = providerTypes.map(pt =>
                `<option value="${pt}" ${pt === spec.provider ? 'selected' : ''}>${pt}</option>`
            ).join('');

            return `
                <div class="role-row" data-model-role="${role.key}">
                    <div class="role-row-label">${role.label}</div>
                    <div class="role-row-fields">
                        <div class="settings-field">
                            <label>Provider</label>
                            <select data-model-provider="${role.key}">
                                <option value="" ${!spec.provider ? 'selected' : ''}>${role.required ? 'Select...' : 'Inherit'}</option>
                                ${provOptions}
                            </select>
                        </div>
                        <div class="settings-field">
                            <label>Model</label>
                            <div class="model-select-wrap" data-model-wrap="${role.key}">
                                <select data-model-select="${role.key}">
                                    ${spec.model ? `<option value="${escAttr(spec.model)}" selected>${escAttr(spec.model)}</option>` : '<option value="">Select provider first</option>'}
                                </select>
                            </div>
                        </div>
                    </div>
                </div>
            `;
        }).join('');

        return `
            ${roleRows}
            <div class="settings-field" style="margin-top:8px">
                <label>Embedding</label>
                <input type="text" data-path="models.embedding" value="${escAttr(models.embedding || '')}"
                    placeholder="openai/text-embedding-3-small">
                <div class="field-hint">Format: provider/model-name</div>
            </div>
        `;
    },

    renderMemory(mem) {
        const search = mem.search || {};
        return `
            <div class="field-row">
                <div class="settings-field">
                    <label>Observer threshold (tokens)</label>
                    <input type="number" data-path="memory.observer_threshold_tokens"
                        value="${mem.observer_threshold_tokens ?? ''}" placeholder="30000">
                </div>
                <div class="settings-field">
                    <label>Reflector threshold (tokens)</label>
                    <input type="number" data-path="memory.reflector_threshold_tokens"
                        value="${mem.reflector_threshold_tokens ?? ''}" placeholder="40000">
                </div>
            </div>
            <div class="field-row">
                <div class="settings-field">
                    <label>Observer cooldown (seconds)</label>
                    <input type="number" data-path="memory.observer_cooldown_secs"
                        value="${mem.observer_cooldown_secs ?? ''}" placeholder="120">
                </div>
                <div class="settings-field">
                    <label>Force threshold (tokens)</label>
                    <input type="number" data-path="memory.observer_force_threshold_tokens"
                        value="${mem.observer_force_threshold_tokens ?? ''}" placeholder="60000">
                </div>
            </div>
            <div style="margin-top:8px;margin-bottom:6px;font-size:12px;color:var(--text-muted);font-weight:600;text-transform:uppercase;letter-spacing:0.05em">Search</div>
            <div class="field-row">
                <div class="settings-field">
                    <label>Vector weight</label>
                    <input type="number" step="0.1" data-path="memory.search.vector_weight"
                        value="${search.vector_weight ?? ''}" placeholder="0.7">
                </div>
                <div class="settings-field">
                    <label>Text weight</label>
                    <input type="number" step="0.1" data-path="memory.search.text_weight"
                        value="${search.text_weight ?? ''}" placeholder="0.3">
                </div>
            </div>
            <div class="field-row">
                <div class="settings-field">
                    <label>Min score</label>
                    <input type="number" step="0.05" data-path="memory.search.min_score"
                        value="${search.min_score ?? ''}" placeholder="0.35">
                </div>
                <div class="settings-field">
                    <label>Candidate multiplier</label>
                    <input type="number" data-path="memory.search.candidate_multiplier"
                        value="${search.candidate_multiplier ?? ''}" placeholder="4">
                </div>
            </div>
        `;
    },

    renderPulse(pulse) {
        const checked = pulse.enabled !== false;
        return `
            <div class="settings-field">
                <label>
                    Enabled
                    <label class="toggle-switch">
                        <input type="checkbox" data-path="pulse.enabled" ${checked ? 'checked' : ''}>
                        <span class="slider"></span>
                    </label>
                </label>
            </div>
        `;
    },

    renderGateway(gw) {
        return `
            <div class="field-row">
                <div class="settings-field">
                    <label>Bind address</label>
                    <input type="text" data-path="gateway.bind" value="${escAttr(gw.bind || '')}"
                        placeholder="127.0.0.1">
                </div>
                <div class="settings-field">
                    <label>Port</label>
                    <input type="number" data-path="gateway.port" value="${gw.port ?? ''}"
                        placeholder="7700">
                </div>
            </div>
        `;
    },

    renderDiscord(discord) {
        return `
            <div class="settings-field">
                <label>Bot token</label>
                <input type="password" data-path="discord.token" value="${escAttr(discord.token || '')}"
                    placeholder="\${IRONCLAW_DISCORD_TOKEN}">
                <div class="field-hint">Supports \${ENV_VAR} syntax for environment variable references</div>
            </div>
        `;
    },

    renderWebhook(wh) {
        const checked = wh.enabled === true;
        return `
            <div class="settings-field">
                <label>
                    Enabled
                    <label class="toggle-switch">
                        <input type="checkbox" data-path="webhook.enabled" ${checked ? 'checked' : ''}>
                        <span class="slider"></span>
                    </label>
                </label>
            </div>
            <div class="settings-field">
                <label>Secret</label>
                <input type="password" data-path="webhook.secret" value="${escAttr(wh.secret || '')}"
                    placeholder="Authorization bearer token">
            </div>
        `;
    },

    renderSkills(skills) {
        const dirs = skills.dirs || [];
        const items = dirs.map((d, i) => `
            <div class="list-editor-item" data-list-idx="${i}">
                <input type="text" data-list="skills.dirs" data-list-idx="${i}" value="${escAttr(d)}">
                <button class="entry-remove-btn" data-remove-list="skills.dirs" data-list-idx="${i}">&times;</button>
            </div>
        `).join('');
        return `
            <div class="settings-field">
                <label>Extra scan directories</label>
                <div class="field-hint" style="margin-bottom:6px">The workspace skills/ directory is always scanned. Add extra directories here.</div>
                <div id="skills-dirs-list">${items}</div>
                <button class="add-entry-btn" data-add="skills-dir">+ Add directory</button>
            </div>
        `;
    },

    renderRetry(retry) {
        return `
            <div class="field-row">
                <div class="settings-field">
                    <label>Max retries</label>
                    <input type="number" data-path="retry.max_retries" value="${retry.max_retries ?? ''}"
                        placeholder="3">
                </div>
                <div class="settings-field">
                    <label>Initial delay (ms)</label>
                    <input type="number" data-path="retry.initial_delay_ms" value="${retry.initial_delay_ms ?? ''}"
                        placeholder="500">
                </div>
            </div>
            <div class="field-row">
                <div class="settings-field">
                    <label>Max delay (ms)</label>
                    <input type="number" data-path="retry.max_delay_ms" value="${retry.max_delay_ms ?? ''}"
                        placeholder="30000">
                </div>
                <div class="settings-field">
                    <label>Backoff multiplier</label>
                    <input type="number" step="0.1" data-path="retry.backoff_multiplier"
                        value="${retry.backoff_multiplier ?? ''}" placeholder="2.0">
                </div>
            </div>
        `;
    },

    renderBackground(bg) {
        const models = bg.models || {};
        return `
            <div class="field-row">
                <div class="settings-field">
                    <label>Max concurrent</label>
                    <input type="number" data-path="background.max_concurrent"
                        value="${bg.max_concurrent ?? ''}" placeholder="3">
                </div>
                <div class="settings-field">
                    <label>Transcript retention (days)</label>
                    <input type="number" data-path="background.transcript_retention_days"
                        value="${bg.transcript_retention_days ?? ''}" placeholder="30">
                </div>
            </div>
            <div style="margin-top:8px;margin-bottom:6px;font-size:12px;color:var(--text-muted);font-weight:600;text-transform:uppercase;letter-spacing:0.05em">Model tiers</div>
            <div class="settings-field">
                <label>Small</label>
                <input type="text" data-path="background.models.small" value="${escAttr(models.small || '')}"
                    placeholder="Falls back to medium \u2192 main">
            </div>
            <div class="field-row">
                <div class="settings-field">
                    <label>Medium</label>
                    <input type="text" data-path="background.models.medium" value="${escAttr(models.medium || '')}"
                        placeholder="Falls back to large \u2192 main">
                </div>
                <div class="settings-field">
                    <label>Large</label>
                    <input type="text" data-path="background.models.large" value="${escAttr(models.large || '')}"
                        placeholder="Falls back to main">
                </div>
            </div>
        `;
    },

    renderNotifications(notifs) {
        const channels = notifs.channels || {};
        const entries = Object.entries(channels);
        const rows = entries.map(([name, cfg]) => this.renderNotificationRow(name, cfg)).join('');
        return `
            <div id="notification-entries">${rows}</div>
            <button class="add-entry-btn" data-add="notification">+ Add channel</button>
        `;
    },

    renderNotificationRow(name, cfg) {
        const isNtfy = cfg.type === 'ntfy';
        const conditionalFields = isNtfy ? `
            <div class="settings-field">
                <label>URL</label>
                <input type="text" data-notif-field="url" value="${escAttr(cfg.url || '')}"
                    placeholder="https://ntfy.sh">
            </div>
            <div class="settings-field">
                <label>Topic</label>
                <input type="text" data-notif-field="topic" value="${escAttr(cfg.topic || '')}">
            </div>
        ` : `
            <div class="settings-field">
                <label>URL</label>
                <input type="text" data-notif-field="url" value="${escAttr(cfg.url || '')}"
                    placeholder="https://hooks.slack.com/services/...">
            </div>
            <div class="settings-field">
                <label>Method</label>
                <input type="text" data-notif-field="method" value="${escAttr(cfg.method || '')}"
                    placeholder="POST">
            </div>
        `;

        return `
            <div class="entry-row" data-notification-name="${escAttr(name)}">
                <div class="entry-row-fields">
                    <div class="settings-field">
                        <label>Name</label>
                        <input type="text" data-notif-field="name" value="${escAttr(name)}">
                    </div>
                    <div class="settings-field">
                        <label>Type</label>
                        <select data-notif-field="type">
                            <option value="ntfy" ${isNtfy ? 'selected' : ''}>ntfy</option>
                            <option value="webhook" ${!isNtfy ? 'selected' : ''}>webhook</option>
                        </select>
                    </div>
                    ${conditionalFields}
                </div>
                <button class="entry-remove-btn" data-remove-notification="${escAttr(name)}">&times;</button>
            </div>
        `;
    },

    renderMcp(mcp) {
        const servers = mcp.servers || {};
        const entries = Object.entries(servers);

        const serverRows = entries.map(([name, cfg]) => `
            <div class="entry-row" data-mcp-name="${escAttr(name)}">
                <div class="entry-row-fields">
                    <div class="settings-field">
                        <label>Name</label>
                        <input type="text" data-mcp-field="name" value="${escAttr(name)}" readonly
                            style="opacity:0.7;cursor:default">
                    </div>
                    <div class="settings-field">
                        <label>Command</label>
                        <input type="text" data-mcp-field="command" value="${escAttr(cfg.command || '')}" readonly
                            style="opacity:0.7;cursor:default">
                    </div>
                </div>
                <button class="entry-remove-btn" data-remove-mcp="${escAttr(name)}">&times;</button>
            </div>
        `).join('');

        const catalogHtml = this.catalog.length > 0 ? `
            <div style="margin-top:12px;margin-bottom:6px;font-size:12px;color:var(--text-muted);font-weight:600;text-transform:uppercase;letter-spacing:0.05em">Catalog</div>
            ${this.catalog.map((srv, i) => {
                const added = servers[srv.name] !== undefined;
                return `
                    <div class="mcp-item ${added ? 'added' : ''}" data-idx="${i}">
                        <div class="mcp-info">
                            <div class="mcp-name">${escAttr(srv.name)}</div>
                            <div class="mcp-desc">${escAttr(srv.description)}</div>
                        </div>
                        <button class="mcp-add-btn" data-idx="${i}">${added ? 'Added' : 'Add'}</button>
                    </div>
                `;
            }).join('')}
        ` : '';

        return `
            <div id="mcp-server-entries">${serverRows}</div>
            ${catalogHtml}
        `;
    },

    // ── Binding ──────────────────────────────────────────────────────

    bind() {
        // Mode toggle
        document.querySelectorAll('.mode-toggle button').forEach(btn => {
            btn.addEventListener('click', () => {
                const newMode = btn.dataset.mode;
                if (newMode === this.mode) return;
                this.switchMode(newMode);
            });
        });

        // Section collapse/expand
        document.querySelectorAll('.config-section-header').forEach(hdr => {
            hdr.addEventListener('click', () => {
                const section = hdr.closest('.config-section');
                const key = section.dataset.section;
                if (this.openSections.has(key)) {
                    this.openSections.delete(key);
                    section.classList.remove('open');
                } else {
                    this.openSections.add(key);
                    section.classList.add('open');
                }
            });
        });

        // Save / validate / refresh
        const saveBtn = document.getElementById('settings-save');
        const validateBtn = document.getElementById('settings-validate');
        const refreshBtn = document.getElementById('settings-refresh');

        if (saveBtn) saveBtn.addEventListener('click', () => this.save());
        if (validateBtn) validateBtn.addEventListener('click', () => this.validateOnly());
        if (refreshBtn) refreshBtn.addEventListener('click', () => this.refresh());

        // Form field change listeners (only in form mode)
        if (this.mode === 'form') {
            this.bindFormFields();
        }
    },

    bindFormFields() {
        // Simple data-path fields
        document.querySelectorAll('[data-path]').forEach(el => {
            const event = el.type === 'checkbox' ? 'change' : 'input';
            el.addEventListener(event, () => this.onFieldChange(el));
        });

        // Provider add/remove
        const addProviderBtn = document.querySelector('[data-add="provider"]');
        if (addProviderBtn) {
            addProviderBtn.addEventListener('click', () => this.addProvider());
        }
        document.querySelectorAll('[data-remove-provider]').forEach(btn => {
            btn.addEventListener('click', () => {
                const name = btn.dataset.removeProvider;
                this.removeProvider(name);
            });
        });

        // Provider field changes
        document.querySelectorAll('.entry-row[data-provider-name]').forEach(row => {
            row.querySelectorAll('input, select').forEach(el => {
                el.addEventListener('change', () => this.collectProviders());
            });
        });

        // Notification add/remove
        const addNotifBtn = document.querySelector('[data-add="notification"]');
        if (addNotifBtn) {
            addNotifBtn.addEventListener('click', () => this.addNotification());
        }
        document.querySelectorAll('[data-remove-notification]').forEach(btn => {
            btn.addEventListener('click', () => {
                const name = btn.dataset.removeNotification;
                this.removeNotification(name);
            });
        });

        // Notification field changes (incl. type swap)
        document.querySelectorAll('.entry-row[data-notification-name]').forEach(row => {
            const typeSelect = row.querySelector('[data-notif-field="type"]');
            if (typeSelect) {
                typeSelect.addEventListener('change', () => {
                    this.collectNotifications();
                    this.rerender();
                });
            }
            row.querySelectorAll('input').forEach(el => {
                el.addEventListener('change', () => this.collectNotifications());
            });
        });

        // Skills dirs
        const addSkillsDirBtn = document.querySelector('[data-add="skills-dir"]');
        if (addSkillsDirBtn) {
            addSkillsDirBtn.addEventListener('click', () => this.addSkillsDir());
        }
        document.querySelectorAll('[data-remove-list="skills.dirs"]').forEach(btn => {
            btn.addEventListener('click', () => {
                const idx = parseInt(btn.dataset.listIdx, 10);
                this.removeSkillsDir(idx);
            });
        });
        document.querySelectorAll('[data-list="skills.dirs"]').forEach(el => {
            el.addEventListener('input', () => this.collectSkillsDirs());
        });

        // MCP remove
        document.querySelectorAll('[data-remove-mcp]').forEach(btn => {
            btn.addEventListener('click', () => {
                const name = btn.dataset.removeMcp;
                this.removeMcpServer(name);
            });
        });

        // MCP catalog add
        document.querySelectorAll('.mcp-add-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                const idx = parseInt(btn.dataset.idx, 10);
                const srv = this.catalog[idx];
                if (!srv) return;
                this.addMcpFromCatalog(srv);
            });
        });

        // Model role provider/model dropdowns
        this.bindModelSelectors();
    },

    /** Bind model provider selectors and populate model dropdowns. */
    bindModelSelectors() {
        const roles = ['main', 'default', 'observer', 'reflector', 'pulse'];

        for (const role of roles) {
            const provSelect = document.querySelector(`[data-model-provider="${role}"]`);
            const modelSelect = document.querySelector(`[data-model-select="${role}"]`);

            if (!provSelect || !modelSelect) continue;

            // On provider change, fetch models
            provSelect.addEventListener('change', () => {
                const prov = provSelect.value;
                if (!prov) {
                    modelSelect.innerHTML = '<option value="">Select provider first</option>';
                    this.updateModelPath(role, '', '');
                    return;
                }
                const { apiKey, url } = this.getProviderCredentials(prov);
                ModelFetcher.populateSelect(modelSelect, prov, apiKey || null, url || null, null).then(() => {
                    this.updateModelPath(role, prov, modelSelect.value);
                });
            });

            // On model change, update config
            modelSelect.addEventListener('change', () => {
                const prov = provSelect.value;
                this.updateModelPath(role, prov, modelSelect.value);
            });

            // Populate on initial load if provider is selected
            const currentProv = provSelect.value;
            if (currentProv) {
                const models = this.configObj.models || {};
                const spec = this.parseModelSpec(models[role] || '');
                const { apiKey, url } = this.getProviderCredentials(currentProv);
                ModelFetcher.populateSelect(modelSelect, currentProv, apiKey || null, url || null, spec.model);
            }
        }
    },

    /** Update the configObj models path from a provider+model dropdown change. */
    updateModelPath(role, provider, model) {
        const spec = this.buildModelSpec(provider, model);
        if (spec) {
            this.setAtPath(['models', role], spec);
        } else {
            this.removeAtPath(['models', role]);
        }
        this.syncTomlFromObj();
    },

    // ── Field change handler ─────────────────────────────────────────

    onFieldChange(el) {
        const path = el.dataset.path;
        const parts = path.split('.');
        let value;

        if (el.type === 'checkbox') {
            value = el.checked;
        } else if (el.type === 'number') {
            const raw = el.value.trim();
            if (raw === '') {
                // Remove the field if empty
                this.removeAtPath(parts);
                this.syncTomlFromObj();
                return;
            }
            value = raw.includes('.') ? parseFloat(raw) : parseInt(raw, 10);
        } else {
            value = el.value;
            if (value === '') {
                this.removeAtPath(parts);
                this.syncTomlFromObj();
                return;
            }
        }

        this.setAtPath(parts, value);
        this.syncTomlFromObj();
    },

    setAtPath(parts, value) {
        let target = this.configObj;
        for (let i = 0; i < parts.length - 1; i++) {
            if (!target[parts[i]] || typeof target[parts[i]] !== 'object') {
                target[parts[i]] = {};
            }
            target = target[parts[i]];
        }
        target[parts[parts.length - 1]] = value;
    },

    removeAtPath(parts) {
        let target = this.configObj;
        for (let i = 0; i < parts.length - 1; i++) {
            if (!target[parts[i]]) return;
            target = target[parts[i]];
        }
        delete target[parts[parts.length - 1]];
        // Clean up empty parent objects
        this.cleanEmptyParents(parts);
    },

    cleanEmptyParents(parts) {
        for (let depth = parts.length - 1; depth > 0; depth--) {
            let target = this.configObj;
            for (let i = 0; i < depth - 1; i++) {
                if (!target[parts[i]]) return;
                target = target[parts[i]];
            }
            const key = parts[depth - 1];
            if (target[key] && typeof target[key] === 'object' && Object.keys(target[key]).length === 0) {
                delete target[key];
            }
        }
    },

    // ── Dynamic entry management ─────────────────────────────────────

    addProvider() {
        if (!this.configObj.providers) this.configObj.providers = {};
        let name = 'new-provider';
        let n = 1;
        while (this.configObj.providers[name]) {
            name = `new-provider-${n++}`;
        }
        this.configObj.providers[name] = { type: 'anthropic', api_key: '' };
        this.syncTomlFromObj();
        this.rerender();
    },

    removeProvider(name) {
        if (this.configObj.providers) {
            delete this.configObj.providers[name];
            if (Object.keys(this.configObj.providers).length === 0) {
                delete this.configObj.providers;
            }
        }
        this.syncTomlFromObj();
        this.rerender();
    },

    collectProviders() {
        const newProviders = {};
        document.querySelectorAll('.entry-row[data-provider-name]').forEach(row => {
            const nameInput = row.querySelector('[data-provider-field="name"]');
            const typeInput = row.querySelector('[data-provider-field="type"]');
            const apiKeyInput = row.querySelector('[data-provider-field="api_key"]');
            const urlInput = row.querySelector('[data-provider-field="url"]');
            const name = nameInput?.value?.trim();
            if (!name) return;
            const cfg = { type: typeInput?.value || 'anthropic' };
            if (apiKeyInput?.value) cfg.api_key = apiKeyInput.value;
            if (urlInput?.value) cfg.url = urlInput.value;
            newProviders[name] = cfg;
        });
        if (Object.keys(newProviders).length > 0) {
            this.configObj.providers = newProviders;
        } else {
            delete this.configObj.providers;
        }
        // Invalidate model cache when providers change
        ModelFetcher.invalidateAll();
        this.syncTomlFromObj();
    },

    addNotification() {
        if (!this.configObj.notifications) this.configObj.notifications = {};
        if (!this.configObj.notifications.channels) this.configObj.notifications.channels = {};
        let name = 'new-channel';
        let n = 1;
        while (this.configObj.notifications.channels[name]) {
            name = `new-channel-${n++}`;
        }
        this.configObj.notifications.channels[name] = { type: 'ntfy', url: '', topic: '' };
        this.syncTomlFromObj();
        this.rerender();
    },

    removeNotification(name) {
        if (this.configObj.notifications?.channels) {
            delete this.configObj.notifications.channels[name];
            if (Object.keys(this.configObj.notifications.channels).length === 0) {
                delete this.configObj.notifications.channels;
                if (Object.keys(this.configObj.notifications).length === 0) {
                    delete this.configObj.notifications;
                }
            }
        }
        this.syncTomlFromObj();
        this.rerender();
    },

    collectNotifications() {
        const newChannels = {};
        document.querySelectorAll('.entry-row[data-notification-name]').forEach(row => {
            const nameInput = row.querySelector('[data-notif-field="name"]');
            const typeInput = row.querySelector('[data-notif-field="type"]');
            const urlInput = row.querySelector('[data-notif-field="url"]');
            const name = nameInput?.value?.trim();
            if (!name) return;
            const cfg = { type: typeInput?.value || 'ntfy' };
            if (urlInput?.value) cfg.url = urlInput.value;
            if (cfg.type === 'ntfy') {
                const topicInput = row.querySelector('[data-notif-field="topic"]');
                if (topicInput?.value) cfg.topic = topicInput.value;
            } else {
                const methodInput = row.querySelector('[data-notif-field="method"]');
                if (methodInput?.value) cfg.method = methodInput.value;
            }
            newChannels[name] = cfg;
        });
        if (Object.keys(newChannels).length > 0) {
            if (!this.configObj.notifications) this.configObj.notifications = {};
            this.configObj.notifications.channels = newChannels;
        } else if (this.configObj.notifications) {
            delete this.configObj.notifications.channels;
            if (Object.keys(this.configObj.notifications).length === 0) {
                delete this.configObj.notifications;
            }
        }
        this.syncTomlFromObj();
    },

    addSkillsDir() {
        if (!this.configObj.skills) this.configObj.skills = {};
        if (!this.configObj.skills.dirs) this.configObj.skills.dirs = [];
        this.configObj.skills.dirs.push('');
        this.syncTomlFromObj();
        this.rerender();
    },

    removeSkillsDir(idx) {
        if (this.configObj.skills?.dirs) {
            this.configObj.skills.dirs.splice(idx, 1);
            if (this.configObj.skills.dirs.length === 0) {
                delete this.configObj.skills.dirs;
                if (Object.keys(this.configObj.skills).length === 0) {
                    delete this.configObj.skills;
                }
            }
        }
        this.syncTomlFromObj();
        this.rerender();
    },

    collectSkillsDirs() {
        const dirs = [];
        document.querySelectorAll('[data-list="skills.dirs"]').forEach(el => {
            if (el.value.trim()) dirs.push(el.value.trim());
        });
        if (dirs.length > 0) {
            if (!this.configObj.skills) this.configObj.skills = {};
            this.configObj.skills.dirs = dirs;
        } else if (this.configObj.skills) {
            delete this.configObj.skills.dirs;
            if (Object.keys(this.configObj.skills).length === 0) {
                delete this.configObj.skills;
            }
        }
        this.syncTomlFromObj();
    },

    removeMcpServer(name) {
        if (this.configObj.mcp?.servers) {
            delete this.configObj.mcp.servers[name];
            if (Object.keys(this.configObj.mcp.servers).length === 0) {
                delete this.configObj.mcp.servers;
                if (Object.keys(this.configObj.mcp).length === 0) {
                    delete this.configObj.mcp;
                }
            }
        }
        this.syncTomlFromObj();
        this.rerender();
    },

    addMcpFromCatalog(srv) {
        if (!this.configObj.mcp) this.configObj.mcp = {};
        if (!this.configObj.mcp.servers) this.configObj.mcp.servers = {};

        // Toggle off if already added
        if (this.configObj.mcp.servers[srv.name]) {
            this.removeMcpServer(srv.name);
            return;
        }

        const entry = { command: srv.command };
        if (srv.args && srv.args.length > 0) entry.args = srv.args;
        if (srv.env && Object.keys(srv.env).length > 0) {
            entry.env = {};
            for (const [k, v] of Object.entries(srv.env)) {
                entry.env[k] = v || 'YOUR_KEY_HERE';
            }
        }
        this.configObj.mcp.servers[srv.name] = entry;
        this.syncTomlFromObj();
        this.rerender();
    },

    // ── Mode switching ───────────────────────────────────────────────

    switchMode(newMode) {
        if (newMode === 'advanced') {
            // Sync from form fields before switching
            if (this.mode === 'form') {
                this.syncTomlFromObj();
            }
        } else if (newMode === 'form') {
            // Parse the raw TOML from the textarea
            if (this.mode === 'advanced') {
                const editor = document.getElementById('settings-toml');
                if (editor) this.currentToml = editor.value;
                this.parseToml();
            }
        }
        this.mode = newMode;
        // Update toggle buttons
        document.querySelectorAll('.mode-toggle button').forEach(btn => {
            btn.classList.toggle('active', btn.dataset.mode === newMode);
        });
        this.rerender();
    },

    rerender() {
        const inner = document.getElementById('settings-inner');
        if (inner) {
            inner.innerHTML = this.render();
            this.bind();
        }
    },

    // ── Save / validate / refresh ────────────────────────────────────

    async save() {
        let toml;
        if (this.mode === 'advanced') {
            const editor = document.getElementById('settings-toml');
            toml = editor ? editor.value : this.currentToml;
        } else {
            this.syncTomlFromObj();
            toml = this.currentToml;
        }

        const saveBtn = document.getElementById('settings-save');
        if (saveBtn) saveBtn.disabled = true;

        try {
            const res = await fetch('/api/config/raw', {
                method: 'PUT',
                headers: { 'Content-Type': 'text/plain' },
                body: toml
            });
            const data = await res.json();
            if (data.valid) {
                this.showValidation('success', 'Saved. Gateway is reloading...');
                this.currentToml = toml;
                this.invalidate();
            } else {
                this.showValidation('error', data.error || 'Validation failed.');
            }
        } catch (err) {
            this.showValidation('error', 'Network error: ' + err.message);
        }
        if (saveBtn) saveBtn.disabled = false;
    },

    async validateOnly() {
        const toml = (document.getElementById('settings-toml') || {}).value || this.currentToml;
        try {
            const res = await fetch('/api/config/validate', {
                method: 'POST',
                headers: { 'Content-Type': 'text/plain' },
                body: toml
            });
            const data = await res.json();
            if (data.valid) {
                this.showValidation('success', 'Configuration is valid.');
            } else {
                this.showValidation('error', data.error || 'Validation failed.');
            }
        } catch (err) {
            this.showValidation('error', 'Network error: ' + err.message);
        }
    },

    async refresh() {
        try {
            const res = await fetch('/api/config/raw');
            this.currentToml = await res.text();
            this.parseToml();
            this.rerender();
            this.showValidation('success', 'Configuration refreshed from disk.');
        } catch (err) {
            this.showValidation('error', 'Failed to refresh: ' + err.message);
        }
    },

    showValidation(type, message) {
        const el = document.getElementById('settings-validation');
        if (!el) return;
        el.className = 'validation-msg ' + type;
        el.textContent = message;
    }
};

function escAttr(text) {
    const d = document.createElement('div');
    d.textContent = text;
    return d.innerHTML;
}
