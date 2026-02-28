// ── IronClaw Web UI — Chat Client ─────────────────────────────────────
//
// WebSocket client that speaks the IronClaw gateway protocol.
// Handles all ServerMessage variants and renders them into the feed.

const Chat = {
    ws: null,
    msgCounter: 0,
    reconnectDelay: 1000,
    reconnectTimer: null,
    pendingToolCalls: new Map(), // id -> tool-item element
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

        this.loadHistory().then(() => this.connect());
    },

    async loadHistory() {
        try {
            const resp = await fetch('/api/chat/history');
            if (!resp.ok) return;
            const messages = await resp.json();
            if (!messages.length) return;

            // Temporary map to match tool results back to their call elements
            const toolCallItems = new Map();

            for (const msg of messages) {
                const content = msg.content || '';
                switch (msg.role) {
                    case 'user':
                        this.appendUserMessage(content);
                        break;
                    case 'assistant':
                        if (content.trim()) {
                            this.appendAssistantMessage(content);
                        }
                        if (msg.tool_calls && msg.tool_calls.length) {
                            const group = document.createElement('div');
                            group.className = 'tool-group';
                            group.style.display = App.verbose ? '' : 'none';
                            this.feed.appendChild(group);

                            for (const tc of msg.tool_calls) {
                                const item = document.createElement('div');
                                item.className = 'tool-item';

                                const header = document.createElement('div');
                                header.className = 'tool-header';
                                header.innerHTML = `
                                    <span class="tool-chevron">&#9654;</span>
                                    <span class="tool-name">${this.esc(tc.name)}</span>
                                    <span class="tool-status ok">done</span>
                                `;
                                header.addEventListener('click', () => item.classList.toggle('open'));

                                const body = document.createElement('div');
                                body.className = 'tool-body';
                                body.textContent = tc.arguments || '';

                                item.appendChild(header);
                                item.appendChild(body);
                                group.appendChild(item);

                                toolCallItems.set(tc.id, item);
                            }
                        }
                        break;
                    case 'tool':
                        if (msg.tool_call_id) {
                            const item = toolCallItems.get(msg.tool_call_id);
                            if (item) {
                                const body = item.querySelector('.tool-body');
                                if (content) {
                                    body.textContent += '\n─── result ───\n' + content;
                                }
                                toolCallItems.delete(msg.tool_call_id);
                            }
                        }
                        break;
                    // skip 'system' messages — internal, not user-facing
                }
            }

            const divider = document.createElement('div');
            divider.className = 'msg msg-divider';
            divider.textContent = '\u2014 session resumed \u2014';
            this.feed.appendChild(divider);
            this.scrollToBottom();
        } catch {
            // history unavailable — start with empty feed
        }
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

        this.pendingToolCalls.set(msg.id, item);
        this.scrollToBottom();
    },

    handleToolResult(msg) {
        const item = this.pendingToolCalls.get(msg.tool_call_id);
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
            this.pendingToolCalls.delete(msg.tool_call_id);
        }
        this.scrollToBottom();
    },

    // ── Rendering ─────────────────────────────────────────────────────

    appendUserMessage(text) {
        const el = document.createElement('div');
        el.className = 'msg msg-user';
        el.innerHTML = `<div class="msg-content">${this.esc(text)}</div>`;
        this.feed.appendChild(el);
        this.scrollToBottom();
    },

    appendAssistantMessage(text) {
        const el = document.createElement('div');
        el.className = 'msg msg-assistant';
        el.innerHTML = `<div class="msg-content">${this.renderMarkdown(text)}</div>`;
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
            <span class="thinking-bars"><span></span><span></span><span></span></span>
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

        // Links: [text](url)
        html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');

        // Horizontal rules: --- or *** on their own line
        html = html.replace(/\n(---|\*\*\*)\n/g, '\n<hr>\n');

        // Headings: ## through #### (must be at line start)
        html = html.replace(/^#### (.+)$/gm, '<h4>$1</h4>');
        html = html.replace(/^### (.+)$/gm, '<h3>$1</h3>');
        html = html.replace(/^## (.+)$/gm, '<h2>$1</h2>');

        // Blockquotes: > text
        html = html.replace(/^&gt; (.+)$/gm, '<blockquote>$1</blockquote>');
        // Merge consecutive blockquotes
        html = html.replace(/<\/blockquote>\n<blockquote>/g, '<br>');

        // Unordered lists: - item
        html = html.replace(/(^|\n)(- .+(?:\n- .+)*)/g, (_m, pre, block) => {
            const items = block.split('\n').map(line => `<li>${line.replace(/^- /, '')}</li>`).join('');
            return `${pre}<ul>${items}</ul>`;
        });

        // Ordered lists: 1. item
        html = html.replace(/(^|\n)(\d+\. .+(?:\n\d+\. .+)*)/g, (_m, pre, block) => {
            const items = block.split('\n').map(line => `<li>${line.replace(/^\d+\. /, '')}</li>`).join('');
            return `${pre}<ol>${items}</ol>`;
        });

        // Line breaks (outside of pre blocks)
        html = html.replace(/\n/g, '<br>');

        // Fix line breaks inside pre that got doubled
        html = html.replace(/<pre><code>([\s\S]*?)<\/code><\/pre>/g, (_m, inner) => {
            return `<pre><code>${inner.replace(/<br>/g, '\n')}</code></pre>`;
        });

        // Clean up line breaks adjacent to block elements
        html = html.replace(/<br>(<\/?(?:h[234]|blockquote|ul|ol|li|hr|pre)>)/g, '$1');
        html = html.replace(/(<\/?(?:h[234]|blockquote|ul|ol|li|hr|pre)>)<br>/g, '$1');

        return html;
    }
};
