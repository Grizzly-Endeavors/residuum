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

    // Section definitions — sidebar navigation order
    sections: [
        { key: 'runtime',          title: 'Runtime',            icon: '\u2699\uFE0F' },
        { key: 'providers-models', title: 'Providers & Models', icon: '\u26A1' },
        { key: 'memory',           title: 'Memory',             icon: '\u{1F9E0}' },
        { key: 'integrations',     title: 'Integrations',       icon: '\u{1F310}' },
        { key: 'mcp',              title: 'MCP Servers',        icon: '\u{1F50C}' },
    ],

    // Currently active sidebar section
    activeSection: 'runtime',

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

        const sidebar = document.getElementById('settings-sidebar');
        if (sidebar) {
            sidebar.innerHTML = this.renderSidebar();
            this.bindSidebar();
        }

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
        const body = this.renderSectionBody(this.activeSection);
        return `
            ${body}
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

    // ── Sidebar ───────────────────────────────────────────────────────

    renderSidebar() {
        return this.sections.map(s => {
            const isActive = this.mode === 'form' && this.activeSection === s.key;
            const badge = this.getSectionBadge(s.key);
            return `
                <div class="settings-sidebar-item${isActive ? ' active' : ''}" data-sidebar="${s.key}">
                    <span class="settings-sidebar-icon">${s.icon}</span>
                    <span class="settings-sidebar-label">${s.title}</span>
                    ${badge ? `<span class="settings-sidebar-badge" title="${escAttr(badge)}">${escAttr(badge)}</span>` : ''}
                </div>
            `;
        }).join('');
    },

    bindSidebar() {
        document.querySelectorAll('.settings-sidebar-item').forEach(item => {
            item.addEventListener('click', () => {
                const key = item.dataset.sidebar;
                if (this.mode === 'advanced') {
                    // Parse current TOML before switching to form
                    const editor = document.getElementById('settings-toml');
                    if (editor) this.currentToml = editor.value;
                    this.parseToml();
                    this.mode = 'form';
                    document.querySelectorAll('.mode-toggle button').forEach(btn => {
                        btn.classList.toggle('active', btn.dataset.mode === 'form');
                    });
                }
                this.activeSection = key;
                this.rerender();
            });
        });
    },

    getSectionBadge(key) {
        const obj = this.configObj;
        switch (key) {
            case 'providers-models':
                return obj.models?.main || '';
            case 'runtime': {
                const parts = [];
                if (obj.gateway?.port || obj.gateway?.bind) parts.push('Gateway');
                return parts.length > 0 ? parts.join(', ') : '';
            }
            case 'integrations': {
                const parts = [];
                if (obj.discord?.token) parts.push('Discord');
                if (obj.webhook?.enabled) parts.push('Webhook');
                const channels = obj.notifications?.channels || {};
                const chCount = Object.keys(channels).length;
                if (chCount > 0) parts.push(`${chCount} notif`);
                const skillDirs = obj.skills?.dirs || [];
                if (skillDirs.length > 0) parts.push(`${skillDirs.length} skills`);
                return parts.length > 0 ? parts.join(', ') : '';
            }
            case 'mcp': {
                const servers = obj.mcp?.servers || {};
                const count = Object.keys(servers).length;
                return count > 0 ? `${count} servers` : '';
            }
            default:
                return '';
        }
    },

    // ── Section body rendering ───────────────────────────────────────

    renderSectionBody(key) {
        const obj = this.configObj;
        switch (key) {
            case 'providers-models':
                return this.renderProvidersAndModels(obj);
            case 'memory':
                return this.renderMemory(obj.memory || {});
            case 'integrations':
                return this.renderIntegrations(obj);
            case 'runtime':
                return this.renderRuntime(obj);
            case 'mcp':
                return this.renderMcp(obj.mcp || {});
            default:
                return '';
        }
    },

    // ── Providers & Models (composite) ───────────────────────────────

    renderProvidersAndModels(obj) {
        const providers = obj.providers || {};
        const models = obj.models || {};
        const bg = obj.background || {};

        return `
            ${this.renderProvidersSubsection(providers)}
            ${this.renderMainModelSubsection(models)}
            ${this.renderMemoryModelsSubsection(models)}
            ${this.renderBackgroundModelTiersSubsection(bg)}
        `;
    },

    renderProvidersSubsection(providers) {
        const entries = Object.entries(providers);
        const rows = entries.map(([name, cfg]) => this.renderProviderRow(name, cfg)).join('');
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Providers</div>
                <div id="provider-entries">${rows}</div>
                <button class="add-entry-btn" data-add="provider">+ Add provider</button>
            </div>
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
                        ${(() => {
                            const isSecret = (cfg.api_key || '').startsWith('secret:');
                            const keyDisplay = isSecret ? '' : escAttr(cfg.api_key || '');
                            const keyPlaceholder = isSecret ? `Encrypted (${cfg.api_key})` : 'sk-...';
                            const secretRef = isSecret ? escAttr(cfg.api_key) : '';
                            return `<input type="password" data-provider-field="api_key" value="${keyDisplay}"
                                placeholder="${keyPlaceholder}" data-secret-ref="${secretRef}">`;
                        })()}
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

    /** Render the Main Model role-row prominently at the top. */
    renderMainModelSubsection(models) {
        const providerNames = this.getConfiguredProviders();
        const spec = this.parseModelSpec(models.main || '');
        const provOptions = providerNames.map(name =>
            `<option value="${escAttr(name)}" ${name === spec.provider ? 'selected' : ''}>${escAttr(name)}</option>`
        ).join('');

        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Main Model</div>
                <div class="role-row" data-model-role="main">
                    <div class="role-row-fields">
                        <div class="settings-field">
                            <label>Provider</label>
                            <select data-model-provider="main">
                                <option value="" ${!spec.provider ? 'selected' : ''}>Select...</option>
                                ${provOptions}
                            </select>
                        </div>
                        <div class="settings-field">
                            <label>Model</label>
                            <div class="model-select-wrap" data-model-wrap="main">
                                <select data-model-select="main">
                                    ${spec.model ? `<option value="${escAttr(spec.model)}" selected>${escAttr(spec.model)}</option>` : '<option value="">Select provider first</option>'}
                                </select>
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        `;
    },

    /** Render Memory Models sub-section: observer, reflector, embedding. */
    renderMemoryModelsSubsection(models) {
        const providerNames = this.getConfiguredProviders();
        const roles = [
            { key: 'observer', label: 'Observer' },
            { key: 'reflector', label: 'Reflector' },
            { key: 'embedding', label: 'Embedding' },
        ];

        const roleRows = roles.map(role => {
            const spec = this.parseModelSpec(models[role.key] || '');
            const provOptions = providerNames.map(name =>
                `<option value="${escAttr(name)}" ${name === spec.provider ? 'selected' : ''}>${escAttr(name)}</option>`
            ).join('');

            return `
                <div class="role-row" data-model-role="${role.key}">
                    <div class="role-row-label">${role.label}</div>
                    <div class="role-row-fields">
                        <div class="settings-field">
                            <label>Provider</label>
                            <select data-model-provider="${role.key}">
                                <option value="" ${!spec.provider ? 'selected' : ''}>Inherit</option>
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
            <div class="config-subsection">
                <div class="config-subsection-title">Memory Models</div>
                ${roleRows}
            </div>
        `;
    },

    /** Render Background Model Tiers sub-section with provider/model dropdowns. */
    renderBackgroundModelTiersSubsection(bg) {
        const bgModels = bg.models || {};
        const providerNames = this.getConfiguredProviders();
        const tiers = [
            { key: 'bg-small', configKey: 'small', label: 'Small', hint: 'Falls back to medium \u2192 main' },
            { key: 'bg-medium', configKey: 'medium', label: 'Medium', hint: 'Falls back to large \u2192 main' },
            { key: 'bg-large', configKey: 'large', label: 'Large', hint: 'Falls back to main' },
        ];

        const tierRows = tiers.map(tier => {
            const spec = this.parseModelSpec(bgModels[tier.configKey] || '');
            const provOptions = providerNames.map(name =>
                `<option value="${escAttr(name)}" ${name === spec.provider ? 'selected' : ''}>${escAttr(name)}</option>`
            ).join('');

            return `
                <div class="role-row" data-model-role="${tier.key}">
                    <div class="role-row-label">${tier.label}</div>
                    <div class="role-row-fields">
                        <div class="settings-field">
                            <label>Provider</label>
                            <select data-model-provider="${tier.key}">
                                <option value="" ${!spec.provider ? 'selected' : ''}>Inherit</option>
                                ${provOptions}
                            </select>
                        </div>
                        <div class="settings-field">
                            <label>Model</label>
                            <div class="model-select-wrap" data-model-wrap="${tier.key}">
                                <select data-model-select="${tier.key}">
                                    ${spec.model ? `<option value="${escAttr(spec.model)}" selected>${escAttr(spec.model)}</option>` : '<option value="">Select provider first</option>'}
                                </select>
                            </div>
                        </div>
                    </div>
                </div>
            `;
        }).join('');

        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Background Model Tiers</div>
                ${tierRows}
            </div>
        `;
    },

    // ── Memory ───────────────────────────────────────────────────────

    renderMemory(mem) {
        const search = mem.search || {};
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Observer &amp; Reflector</div>
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
            </div>
            <div class="config-subsection">
                <div class="config-subsection-title">Search</div>
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
            </div>
        `;
    },

    // ── Integrations (composite) ─────────────────────────────────────

    renderIntegrations(obj) {
        const discord = obj.discord || {};
        const wh = obj.webhook || {};
        const notifs = obj.notifications || {};

        return `
            ${this.renderDiscordSubsection(discord)}
            ${this.renderWebhookSubsection(wh)}
            ${this.renderNotificationsSubsection(notifs)}
            ${this.renderSkillsSubsection(obj.skills || {})}
        `;
    },

    renderGatewaySubsection(gw) {
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Gateway</div>
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
            </div>
        `;
    },

    renderDiscordSubsection(discord) {
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Discord</div>
                <div class="settings-field">
                    <label>Bot token</label>
                    ${(() => {
                        const isSecret = (discord.token || '').startsWith('secret:');
                        const tokenDisplay = isSecret ? '' : escAttr(discord.token || '');
                        const tokenPlaceholder = isSecret ? `Encrypted (${discord.token})` : '${IRONCLAW_DISCORD_TOKEN}';
                        const secretRef = isSecret ? escAttr(discord.token) : '';
                        return `<input type="password" data-path="discord.token" value="${tokenDisplay}"
                            placeholder="${tokenPlaceholder}" data-secret-ref="${secretRef}">`;
                    })()}
                    <div class="field-hint">Supports \${ENV_VAR} syntax for environment variable references</div>
                </div>
            </div>
        `;
    },

    renderWebhookSubsection(wh) {
        const checked = wh.enabled === true;
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Webhook</div>
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
                    ${(() => {
                        const isSecret = (wh.secret || '').startsWith('secret:');
                        const secretDisplay = isSecret ? '' : escAttr(wh.secret || '');
                        const secretPlaceholder = isSecret ? `Encrypted (${wh.secret})` : 'Authorization bearer token';
                        const secretRef = isSecret ? escAttr(wh.secret) : '';
                        return `<input type="password" data-path="webhook.secret" value="${secretDisplay}"
                            placeholder="${secretPlaceholder}" data-secret-ref="${secretRef}">`;
                    })()}
                </div>
            </div>
        `;
    },

    renderNotificationsSubsection(notifs) {
        const channels = notifs.channels || {};
        const entries = Object.entries(channels);
        const rows = entries.map(([name, cfg]) => this.renderNotificationRow(name, cfg)).join('');
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Notification Channels</div>
                <div id="notification-entries">${rows}</div>
                <button class="add-entry-btn" data-add="notification">+ Add channel</button>
            </div>
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

    // ── Runtime (composite) ──────────────────────────────────────────

    renderRuntime(obj) {
        return `
            ${this.renderGeneralSubsection(obj)}
            ${this.renderGatewaySubsection(obj.gateway || {})}
            ${this.renderPulseSubsection(obj.pulse || {})}
            ${this.renderBackgroundOpsSubsection(obj.background || {})}
            ${this.renderRetrySubsection(obj.retry || {})}
        `;
    },

    renderGeneralSubsection(obj) {
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">General</div>
                <div class="settings-field">
                    <label>Timezone (IANA format)</label>
                    <input type="text" data-path="timezone" value="${escAttr(obj.timezone || '')}"
                        placeholder="America/New_York">
                </div>
                <div class="settings-field">
                    <label>Workspace directory</label>
                    <input type="text" data-path="workspace_dir" value="${escAttr(obj.workspace_dir || '')}"
                        placeholder="~/.ironclaw/workspace">
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
            </div>
        `;
    },

    renderSkillsSubsection(skills) {
        const dirs = skills.dirs || [];
        const items = dirs.map((d, i) => `
            <div class="list-editor-item" data-list-idx="${i}">
                <input type="text" data-list="skills.dirs" data-list-idx="${i}" value="${escAttr(d)}">
                <button class="entry-remove-btn" data-remove-list="skills.dirs" data-list-idx="${i}">&times;</button>
            </div>
        `).join('');
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Skills</div>
                <div class="settings-field">
                    <label>Extra scan directories</label>
                    <div class="field-hint" style="margin-bottom:6px">The workspace skills/ directory is always scanned. Add extra directories here.</div>
                    <div id="skills-dirs-list">${items}</div>
                    <button class="add-entry-btn" data-add="skills-dir">+ Add directory</button>
                </div>
            </div>
        `;
    },

    renderPulseSubsection(pulse) {
        const checked = pulse.enabled !== false;
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Pulse</div>
                <div class="settings-field">
                    <label>
                        Enabled
                        <label class="toggle-switch">
                            <input type="checkbox" data-path="pulse.enabled" ${checked ? 'checked' : ''}>
                            <span class="slider"></span>
                        </label>
                    </label>
                </div>
            </div>
        `;
    },

    renderBackgroundOpsSubsection(bg) {
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Background Tasks</div>
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
            </div>
        `;
    },

    renderRetrySubsection(retry) {
        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Retry Policy</div>
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
            </div>
        `;
    },

    // ── MCP Servers ──────────────────────────────────────────────────

    renderMcp(mcp) {
        const servers = mcp.servers || {};
        const entries = Object.entries(servers);

        const serverRows = entries.map(([name, cfg]) => this.renderMcpServerRow(name, cfg)).join('');

        const catalogHtml = this.catalog.length > 0 ? `
            <div class="config-subsection">
                <div class="config-subsection-title">Catalog</div>
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
            </div>
        ` : '';

        return `
            <div class="config-subsection">
                <div class="config-subsection-title">Configured Servers</div>
                <div id="mcp-server-entries">${serverRows}</div>
                <button class="add-entry-btn" data-add="mcp-server">+ Add server</button>
            </div>
            ${catalogHtml}
        `;
    },

    renderMcpServerRow(name, cfg) {
        const isManual = cfg._manual === true;
        const readonlyAttr = isManual ? '' : 'readonly';
        const readonlyStyle = isManual ? '' : 'style="opacity:0.7;cursor:default"';
        const argsStr = Array.isArray(cfg.args) ? cfg.args.join(' ') : '';
        const envStr = cfg.env ? Object.entries(cfg.env).map(([k, v]) => `${k}=${v}`).join(', ') : '';

        return `
            <div class="entry-row" data-mcp-name="${escAttr(name)}" ${isManual ? 'data-mcp-manual="true"' : ''}>
                <div class="entry-row-fields">
                    <div class="settings-field">
                        <label>Name</label>
                        <input type="text" data-mcp-field="name" value="${escAttr(name)}"
                            ${readonlyAttr} ${readonlyStyle}>
                    </div>
                    <div class="settings-field">
                        <label>Command</label>
                        <input type="text" data-mcp-field="command" value="${escAttr(cfg.command || '')}"
                            ${readonlyAttr} ${readonlyStyle}>
                    </div>
                    <div class="settings-field">
                        <label>Args</label>
                        <input type="text" data-mcp-field="args" value="${escAttr(argsStr)}"
                            ${readonlyAttr} ${readonlyStyle} placeholder="space-separated">
                    </div>
                    <div class="settings-field">
                        <label>Env</label>
                        <input type="text" data-mcp-field="env" value="${escAttr(envStr)}"
                            ${readonlyAttr} ${readonlyStyle} placeholder="KEY=value, KEY2=value2">
                    </div>
                </div>
                <button class="entry-remove-btn" data-remove-mcp="${escAttr(name)}">&times;</button>
            </div>
        `;
    },

    // ── Helpers ───────────────────────────────────────────────────────

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

    /** Get configured provider names (keys from [providers] table). */
    getConfiguredProviders() {
        const providers = this.configObj.providers || {};
        return Object.keys(providers).sort();
    },

    /** Look up a configured provider by name, returning its type, API key, and URL. */
    getProviderCredentials(providerName) {
        const providers = this.configObj.providers || {};
        const cfg = providers[providerName];
        if (cfg) {
            return {
                type: cfg.type || providerName,
                apiKey: cfg.api_key || '',
                url: cfg.url || '',
            };
        }
        return { type: providerName, apiKey: '', url: '' };
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

        // MCP manual server add
        const addMcpBtn = document.querySelector('[data-add="mcp-server"]');
        if (addMcpBtn) {
            addMcpBtn.addEventListener('click', () => this.addMcpServer());
        }

        // MCP manual server field changes
        document.querySelectorAll('.entry-row[data-mcp-manual="true"]').forEach(row => {
            row.querySelectorAll('input').forEach(el => {
                el.addEventListener('change', () => this.collectMcpServers());
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
        const roles = ['main', 'observer', 'reflector', 'embedding', 'bg-small', 'bg-medium', 'bg-large'];

        for (const role of roles) {
            const provSelect = document.querySelector(`[data-model-provider="${role}"]`);
            const modelSelect = document.querySelector(`[data-model-select="${role}"]`);

            if (!provSelect || !modelSelect) continue;

            // On provider change, fetch models using the provider's type
            provSelect.addEventListener('change', () => {
                const provName = provSelect.value;
                if (!provName) {
                    modelSelect.innerHTML = '<option value="">Select provider first</option>';
                    this.updateModelPath(role, '', '');
                    return;
                }
                const { type, apiKey, url } = this.getProviderCredentials(provName);
                ModelFetcher.populateSelect(modelSelect, type, apiKey || null, url || null, null).then(() => {
                    this.updateModelPath(role, provName, ModelFetcher.getSelectedModel(modelSelect));
                });
            });

            // On model change (select or "other" text input), update config
            modelSelect.addEventListener('change', () => {
                const provName = provSelect.value;
                this.updateModelPath(role, provName, ModelFetcher.getSelectedModel(modelSelect));
            });

            // Listen for typing in the "other" text input
            const wrap = modelSelect.closest('.model-select-wrap') || modelSelect.parentElement;
            const observer = new MutationObserver(() => {
                const otherInput = wrap.querySelector('.model-other-input');
                if (otherInput && !otherInput._settingsBound) {
                    otherInput._settingsBound = true;
                    otherInput.addEventListener('input', () => {
                        const provName = provSelect.value;
                        this.updateModelPath(role, provName, otherInput.value.trim());
                    });
                }
            });
            observer.observe(wrap, { childList: true });

            // Populate on initial load if provider is selected
            const currentProvName = provSelect.value;
            if (currentProvName) {
                const currentModel = this._getModelForRole(role);
                const spec = this.parseModelSpec(currentModel);
                const { type, apiKey, url } = this.getProviderCredentials(currentProvName);
                ModelFetcher.populateSelect(modelSelect, type, apiKey || null, url || null, spec.model);
            }
        }
    },

    /** Get the current model spec string for a given role key (handles bg-* mapping). */
    _getModelForRole(role) {
        if (role.startsWith('bg-')) {
            const tier = role.substring(3); // 'small', 'medium', 'large'
            return (this.configObj.background?.models || {})[tier] || '';
        }
        return (this.configObj.models || {})[role] || '';
    },

    /** Update the configObj models path from a provider+model dropdown change. */
    updateModelPath(role, provider, model) {
        const spec = this.buildModelSpec(provider, model);

        if (role.startsWith('bg-')) {
            const tier = role.substring(3); // 'small', 'medium', 'large'
            if (spec) {
                this.setAtPath(['background', 'models', tier], spec);
            } else {
                this.removeAtPath(['background', 'models', tier]);
            }
        } else {
            if (spec) {
                this.setAtPath(['models', role], spec);
            } else {
                this.removeAtPath(['models', role]);
            }
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
                // Preserve secret: references when field is empty (user didn't change it)
                if (el.dataset?.secretRef) {
                    value = el.dataset.secretRef;
                } else {
                    this.removeAtPath(parts);
                    this.syncTomlFromObj();
                    return;
                }
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
            if (apiKeyInput?.value) {
                cfg.api_key = apiKeyInput.value;
            } else if (apiKeyInput?.dataset?.secretRef) {
                cfg.api_key = apiKeyInput.dataset.secretRef;
            }
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

    addMcpServer() {
        if (!this.configObj.mcp) this.configObj.mcp = {};
        if (!this.configObj.mcp.servers) this.configObj.mcp.servers = {};
        let name = 'new-server';
        let n = 1;
        while (this.configObj.mcp.servers[name]) {
            name = `new-server-${n++}`;
        }
        this.configObj.mcp.servers[name] = { command: '', _manual: true };
        this.syncTomlFromObj();
        this.rerender();
    },

    /** Collect MCP servers from manually-editable rows back into configObj. */
    collectMcpServers() {
        document.querySelectorAll('.entry-row[data-mcp-manual="true"]').forEach(row => {
            const origName = row.dataset.mcpName;
            const nameInput = row.querySelector('[data-mcp-field="name"]');
            const cmdInput = row.querySelector('[data-mcp-field="command"]');
            const argsInput = row.querySelector('[data-mcp-field="args"]');
            const envInput = row.querySelector('[data-mcp-field="env"]');

            const newName = nameInput?.value?.trim();
            const command = cmdInput?.value?.trim() || '';
            if (!newName) return;

            // Build the server entry
            const entry = { command, _manual: true };

            // Parse args from space-separated string
            const argsStr = argsInput?.value?.trim() || '';
            if (argsStr) {
                entry.args = argsStr.split(/\s+/);
            }

            // Parse env from "KEY=value, KEY2=value2" string
            const envStr = envInput?.value?.trim() || '';
            if (envStr) {
                const env = {};
                for (const pair of envStr.split(',')) {
                    const eqIdx = pair.indexOf('=');
                    if (eqIdx > 0) {
                        env[pair.substring(0, eqIdx).trim()] = pair.substring(eqIdx + 1).trim();
                    }
                }
                if (Object.keys(env).length > 0) entry.env = env;
            }

            // If name changed, remove old entry
            if (origName !== newName && this.configObj.mcp?.servers) {
                delete this.configObj.mcp.servers[origName];
            }

            if (!this.configObj.mcp) this.configObj.mcp = {};
            if (!this.configObj.mcp.servers) this.configObj.mcp.servers = {};
            this.configObj.mcp.servers[newName] = entry;
        });
        this.syncTomlFromObj();
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
        const sidebar = document.getElementById('settings-sidebar');
        if (sidebar) {
            sidebar.innerHTML = this.renderSidebar();
            this.bindSidebar();
        }
        const inner = document.getElementById('settings-inner');
        if (inner) {
            inner.innerHTML = this.render();
            this.bind();
        }
    },

    /** Store any new raw API keys (not already secret: or ${) as encrypted secrets. */
    async storeNewSecrets() {
        const promises = [];

        // Provider API keys
        const providers = this.configObj.providers || {};
        for (const [name, cfg] of Object.entries(providers)) {
            if (cfg.api_key && !cfg.api_key.startsWith('secret:') && !cfg.api_key.startsWith('${')) {
                promises.push(
                    fetch('/api/secrets', {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ name, value: cfg.api_key })
                    })
                    .then(r => r.json())
                    .then(data => { cfg.api_key = data.reference; })
                );
            }
        }

        // Discord token
        const discordToken = this.configObj.discord?.token;
        if (discordToken && !discordToken.startsWith('secret:') && !discordToken.startsWith('${')) {
            promises.push(
                fetch('/api/secrets', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ name: 'discord_token', value: discordToken })
                })
                .then(r => r.json())
                .then(data => { this.configObj.discord.token = data.reference; })
            );
        }

        // Webhook secret
        const webhookSecret = this.configObj.webhook?.secret;
        if (webhookSecret && !webhookSecret.startsWith('secret:') && !webhookSecret.startsWith('${')) {
            promises.push(
                fetch('/api/secrets', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ name: 'webhook_secret', value: webhookSecret })
                })
                .then(r => r.json())
                .then(data => { this.configObj.webhook.secret = data.reference; })
            );
        }

        await Promise.all(promises);
        this.syncTomlFromObj();
    },

    // ── Save / validate / refresh ────────────────────────────────────

    async save() {
        let toml;
        if (this.mode === 'advanced') {
            const editor = document.getElementById('settings-toml');
            toml = editor ? editor.value : this.currentToml;
        } else {
            // Strip internal _manual markers before saving
            this._stripInternalMarkers();
            // Store new raw keys as encrypted secrets
            try {
                await this.storeNewSecrets();
            } catch (err) {
                this.showValidation('error', 'Failed to store secrets: ' + err.message);
                return;
            }
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

    /** Remove internal _manual markers from MCP server entries before serialization. */
    _stripInternalMarkers() {
        const servers = this.configObj.mcp?.servers;
        if (!servers) return;
        for (const cfg of Object.values(servers)) {
            delete cfg._manual;
        }
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
