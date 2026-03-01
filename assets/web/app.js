// ── Residuum Web UI — Entry Point ─────────────────────────────────────
//
// Fetches /api/status to determine mode (setup vs running), then
// initializes the appropriate view.

const App = {
    mode: null,
    verbose: localStorage.getItem('residuum-verbose') === 'true',

    async init() {
        try {
            const res = await fetch('/api/status');
            const data = await res.json();
            this.mode = data.mode;
        } catch {
            this.mode = 'running'; // assume running if status fails
        }

        if (this.mode === 'setup') {
            this.showSetup();
        } else {
            this.showChat();
        }

        this.bindGlobalControls();
    },

    showSetup() {
        document.getElementById('setup-view').classList.add('active');
        document.getElementById('chat-view').classList.add('hidden');
        document.getElementById('settings-view').classList.remove('active');
        document.getElementById('btn-settings').style.display = 'none';
        document.getElementById('btn-verbose').style.display = 'none';
        document.getElementById('conn-status').textContent = 'setup';
        Setup.init();
    },

    showChat() {
        document.getElementById('setup-view').classList.remove('active');
        document.getElementById('chat-view').classList.remove('hidden');
        document.getElementById('settings-view').classList.remove('active');
        document.getElementById('btn-settings').style.display = '';
        document.getElementById('btn-verbose').style.display = '';
        Chat.init();
    },

    showSettings() {
        document.getElementById('chat-view').classList.add('hidden');
        document.getElementById('settings-view').classList.add('active');
        Settings.init();
    },

    hideSettings() {
        document.getElementById('settings-view').classList.remove('active');
        document.getElementById('chat-view').classList.remove('hidden');
    },

    bindGlobalControls() {
        document.getElementById('btn-settings').addEventListener('click', () => {
            const sv = document.getElementById('settings-view');
            if (sv.classList.contains('active')) {
                this.hideSettings();
            } else {
                this.showSettings();
            }
        });

        document.getElementById('btn-settings-back').addEventListener('click', () => {
            this.hideSettings();
        });

        const verboseBtn = document.getElementById('btn-verbose');
        verboseBtn.classList.toggle('active', this.verbose);

        verboseBtn.addEventListener('click', () => {
            this.verbose = !this.verbose;
            localStorage.setItem('residuum-verbose', this.verbose);
            verboseBtn.classList.toggle('active', this.verbose);
            Chat.setVerbose(this.verbose);
        });

        document.getElementById('btn-reload').addEventListener('click', () => {
            Chat.sendReload();
        });
    },

    // Called by setup wizard when config is saved
    onSetupComplete() {
        document.getElementById('setup-view').classList.remove('active');
        this.mode = 'running';
        this.showChat();
    }
};

document.addEventListener('DOMContentLoaded', () => App.init());
