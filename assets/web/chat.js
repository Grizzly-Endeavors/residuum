// ── IronClaw Web UI — Chat Client ─────────────────────────────────────
//
// WebSocket client that speaks the IronClaw gateway protocol.
// Handles all ServerMessage variants and renders them into the feed.

const Chat = {
    ws: null,
    msgCounter: 0,
    reconnectDelay: 1000,
    reconnectTimer: null,
    pendingToolCalls: new Map(), // name -> tool-item element
    thinkingEl: null,
    isProcessing: false,

    init() {
        this.feed = document.getElementById('chat-feed-inner');
        this.input = document.getElementById('chat-input');
        this.sendBtn = document.getElementById('send-btn');

        this.input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                this.send();
            }
        });

        // Auto-resize textarea
        this.input.addEventListener('input', () => {
            this.input.style.height = 'auto';
            this.input.style.height = Math.min(this.input.scrollHeight, 160) + 'px';
        });

        this.sendBtn.addEventListener('click', () => this.send());

        this.connect();
    },

    connect() {
        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const url = `${proto}//${location.host}/ws`;

        this.setStatus('connecting');
        this.ws = new WebSocket(url);

        this.ws.onopen = () => {
            this.setStatus('connected');
            this.reconnectDelay = 1000;
            if (App.verbose) {
                this.ws.send(JSON.stringify({ type: 'set_verbose', enabled: true }));
            }
        };

        this.ws.onmessage = (e) => {
            try {
                const msg = JSON.parse(e.data);
                this.handleMessage(msg);
            } catch { /* ignore unparseable frames */ }
        };

        this.ws.onclose = () => {
            this.setStatus('disconnected');
            this.scheduleReconnect();
        };

        this.ws.onerror = () => {
            this.setStatus('disconnected');
        };
    },

    scheduleReconnect() {
        if (this.reconnectTimer) return;
        this.reconnectTimer = setTimeout(() => {
            this.reconnectTimer = null;
            this.connect();
        }, this.reconnectDelay);
        this.reconnectDelay = Math.min(this.reconnectDelay * 1.5, 15000);
    },

    setStatus(state) {
        const el = document.getElementById('conn-status');
        el.className = 'header-status ' + state;
        el.textContent = state;
    },

    send() {
        const text = this.input.value.trim();
        if (!text || !this.ws || this.ws.readyState !== WebSocket.OPEN) return;

        this.msgCounter++;
        const id = 'web-' + this.msgCounter;
        const msg = { type: 'send_message', id, content: text };

        this.ws.send(JSON.stringify(msg));
        this.appendUserMessage(text);
        this.input.value = '';
        this.input.style.height = 'auto';
        this.showThinking();
    },

    setVerbose(enabled) {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify({ type: 'set_verbose', enabled }));
        }
        // Toggle visibility of existing tool groups
        this.feed.querySelectorAll('.tool-group').forEach(el => {
            el.style.display = enabled ? '' : 'none';
        });
    },

    sendReload() {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify({ type: 'reload' }));
        }
    },

    // ── Message handling ──────────────────────────────────────────────

    handleMessage(msg) {
        switch (msg.type) {
            case 'turn_started':
                this.isProcessing = true;
                this.showThinking();
                break;

            case 'tool_call':
                this.handleToolCall(msg);
                break;

            case 'tool_result':
                this.handleToolResult(msg);
                break;

            case 'response':
                this.hideThinking();
                this.isProcessing = false;
                if (msg.content) {
                    this.appendAssistantMessage(msg.content);
                }
                break;

            case 'broadcast_response':
                if (msg.content) {
                    this.appendAssistantMessage(msg.content);
                }
                break;

            case 'system_event':
                this.appendSystemMessage(`[${msg.source}] ${msg.content}`);
                break;

            case 'error':
                this.hideThinking();
                this.isProcessing = false;
                this.appendError(msg.message);
                break;

            case 'notice':
                this.appendNotice(msg.message);
                break;

            case 'reloading':
                this.appendSystemMessage('Gateway is reloading...');
                break;

            case 'pong':
                break; // silent
        }
    },

    handleToolCall(msg) {
        // Ensure a tool group exists for the current turn
        let group = this.feed.querySelector('.tool-group:last-child');
        if (!group || group.nextElementSibling) {
            group = document.createElement('div');
            group.className = 'tool-group';
            group.style.display = App.verbose ? '' : 'none';
            this.feed.appendChild(group);
        }

        const item = document.createElement('div');
        item.className = 'tool-item';

        const header = document.createElement('div');
        header.className = 'tool-header';
        header.innerHTML = `
            <span class="tool-chevron">&#9654;</span>
            <span class="tool-name">${this.esc(msg.name)}</span>
            <span class="tool-status">running...</span>
        `;
        header.addEventListener('click', () => item.classList.toggle('open'));

        const body = document.createElement('div');
        body.className = 'tool-body';
        const argsText = typeof msg.arguments === 'string'
            ? msg.arguments
            : JSON.stringify(msg.arguments, null, 2);
        body.textContent = argsText;

        item.appendChild(header);
        item.appendChild(body);
        group.appendChild(item);

        this.pendingToolCalls.set(msg.name, item);
        this.scrollToBottom();
    },

    handleToolResult(msg) {
        const item = this.pendingToolCalls.get(msg.name);
        if (item) {
            const status = item.querySelector('.tool-status');
            if (msg.is_error) {
                status.className = 'tool-status err';
                status.textContent = 'error';
            } else {
                status.className = 'tool-status ok';
                status.textContent = 'done';
            }
            // Append result to body
            const body = item.querySelector('.tool-body');
            if (msg.output) {
                body.textContent += '\n─── result ───\n' + msg.output;
            }
            this.pendingToolCalls.delete(msg.name);
        }
        this.scrollToBottom();
    },

    // ── Rendering ─────────────────────────────────────────────────────

    appendUserMessage(text) {
        const el = document.createElement('div');
        el.className = 'msg msg-user';
        el.innerHTML = `<div class="msg-label">You</div><div class="msg-content">${this.esc(text)}</div>`;
        this.feed.appendChild(el);
        this.scrollToBottom();
    },

    appendAssistantMessage(text) {
        const el = document.createElement('div');
        el.className = 'msg msg-assistant';
        el.innerHTML = `<div class="msg-label">IronClaw</div><div class="msg-content">${this.renderMarkdown(text)}</div>`;
        this.feed.appendChild(el);
        this.scrollToBottom();
    },

    appendSystemMessage(text) {
        const el = document.createElement('div');
        el.className = 'msg msg-system';
        el.textContent = text;
        this.feed.appendChild(el);
        this.scrollToBottom();
    },

    appendError(text) {
        const el = document.createElement('div');
        el.className = 'msg msg-error';
        el.textContent = text;
        this.feed.appendChild(el);
        this.scrollToBottom();
    },

    appendNotice(text) {
        const el = document.createElement('div');
        el.className = 'msg msg-notice';
        el.textContent = text;
        this.feed.appendChild(el);
        this.scrollToBottom();
    },

    showThinking() {
        if (this.thinkingEl) return;
        this.thinkingEl = document.createElement('div');
        this.thinkingEl.className = 'thinking';
        this.thinkingEl.innerHTML = `
            <span class="thinking-dots"><span></span><span></span><span></span></span>
            <span>Thinking...</span>
        `;
        this.feed.appendChild(this.thinkingEl);
        this.scrollToBottom();
    },

    hideThinking() {
        if (this.thinkingEl) {
            this.thinkingEl.remove();
            this.thinkingEl = null;
        }
    },

    scrollToBottom() {
        const feedEl = document.getElementById('chat-feed');
        requestAnimationFrame(() => {
            feedEl.scrollTop = feedEl.scrollHeight;
        });
    },

    // ── Text rendering ────────────────────────────────────────────────

    esc(text) {
        const d = document.createElement('div');
        d.textContent = text;
        return d.innerHTML;
    },

    renderMarkdown(text) {
        // Escape HTML first
        let html = this.esc(text);

        // Code blocks: ```lang\n...\n```
        html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_m, _lang, code) => {
            return `<pre><code>${code}</code></pre>`;
        });

        // Inline code: `...`
        html = html.replace(/`([^`]+)`/g, '<code>$1</code>');

        // Bold: **...**
        html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');

        // Italic: *...*
        html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, '<em>$1</em>');

        // Line breaks (outside of pre blocks)
        html = html.replace(/\n/g, '<br>');

        // Fix line breaks inside pre that got doubled
        html = html.replace(/<pre><code>([\s\S]*?)<\/code><\/pre>/g, (_m, inner) => {
            return `<pre><code>${inner.replace(/<br>/g, '\n')}</code></pre>`;
        });

        return html;
    }
};
