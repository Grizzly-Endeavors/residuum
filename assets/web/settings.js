// ── IronClaw Web UI — Settings Panel ──────────────────────────────────
//
// Config editing with raw TOML editor and validate/save workflow.
// Also provides MCP catalog browsing for adding servers.

const Settings = {
    initialized: false,
    currentToml: '',
    catalog: [],

    async init() {
        if (this.initialized) return;
        this.initialized = true;

        const inner = document.getElementById('settings-inner');
        inner.innerHTML = this.renderSkeleton();

        // Load current config and catalog in parallel
        const [tomlRes, catalogRes] = await Promise.all([
            fetch('/api/config/raw').then(r => r.text()).catch(() => ''),
            fetch('/api/mcp-catalog').then(r => r.json()).catch(() => [])
        ]);

        this.currentToml = tomlRes;
        this.catalog = catalogRes;

        inner.innerHTML = this.render();
        this.bind();
    },

    // Force re-fetch on next open
    invalidate() {
        this.initialized = false;
    },

    renderSkeleton() {
        return '<p style="color:var(--text-dim);padding:20px">Loading configuration...</p>';
    },

    render() {
        const catalogHtml = this.catalog.map((srv, i) => `
            <div class="mcp-item" data-idx="${i}">
                <div class="mcp-info">
                    <div class="mcp-name">${esc(srv.name)}</div>
                    <div class="mcp-desc">${esc(srv.description)}</div>
                </div>
                <button class="mcp-add-btn btn btn-secondary" data-idx="${i}">Add to config</button>
            </div>
        `).join('');

        return `
            <div class="settings-section">
                <h3>Configuration</h3>
                <p style="color:var(--text-muted);font-size:12px;margin-bottom:12px">
                    Edit the raw TOML configuration. Changes are validated before saving.
                    Saving triggers a gateway reload.
                </p>
                <textarea class="toml-editor" id="settings-toml">${escAttr(this.currentToml)}</textarea>
                <div class="validation-msg" id="settings-validation"></div>
                <div class="settings-actions">
                    <button class="btn btn-primary" id="settings-save">Validate &amp; Save</button>
                    <button class="btn btn-secondary" id="settings-validate">Validate Only</button>
                    <button class="btn btn-secondary" id="settings-refresh">Refresh</button>
                </div>
            </div>

            ${this.catalog.length > 0 ? `
                <div class="settings-section">
                    <h3>MCP Server Catalog</h3>
                    <p style="color:var(--text-muted);font-size:12px;margin-bottom:12px">
                        Click "Add to config" to append a server entry to your TOML. Fill in any
                        required fields (API keys, paths) after adding.
                    </p>
                    ${catalogHtml}
                </div>
            ` : ''}

            <div class="settings-section">
                <h3>Reference</h3>
                <p style="color:var(--text-muted);font-size:12px">
                    See <code>config.example.toml</code> in your config directory for all available options.
                </p>
            </div>
        `;
    },

    bind() {
        const saveBtn = document.getElementById('settings-save');
        const validateBtn = document.getElementById('settings-validate');
        const refreshBtn = document.getElementById('settings-refresh');

        if (saveBtn) saveBtn.addEventListener('click', () => this.save());
        if (validateBtn) validateBtn.addEventListener('click', () => this.validate());
        if (refreshBtn) refreshBtn.addEventListener('click', () => this.refresh());

        // MCP catalog add buttons
        document.querySelectorAll('#settings-inner .mcp-add-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                const idx = parseInt(btn.dataset.idx, 10);
                const srv = this.catalog[idx];
                if (!srv) return;
                this.addMcpToToml(srv);
            });
        });
    },

    addMcpToToml(srv) {
        const editor = document.getElementById('settings-toml');
        if (!editor) return;

        let snippet = `\n[mcp.servers.${srv.name}]\ncommand = "${srv.command}"`;
        if (srv.args && srv.args.length > 0) {
            const argsStr = srv.args.map(a => `"${a}"`).join(', ');
            snippet += `\nargs = [${argsStr}]`;
        }
        if (srv.env && Object.keys(srv.env).length > 0) {
            const envParts = Object.entries(srv.env)
                .map(([k, v]) => `${k} = "${v || 'YOUR_KEY_HERE'}"`).join(', ');
            snippet += `\nenv = { ${envParts} }`;
        }
        if (srv.install_hint) {
            snippet += `\n# ${srv.install_hint}`;
        }
        snippet += '\n';

        editor.value += snippet;
        editor.scrollTop = editor.scrollHeight;
        this.showValidation('success', `Added ${srv.name} — fill in any placeholder values above.`);
    },

    async validate() {
        const toml = (document.getElementById('settings-toml') || {}).value || '';
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

    async save() {
        const toml = (document.getElementById('settings-toml') || {}).value || '';
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

    async refresh() {
        try {
            const res = await fetch('/api/config/raw');
            const toml = await res.text();
            this.currentToml = toml;
            const editor = document.getElementById('settings-toml');
            if (editor) editor.value = toml;
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
