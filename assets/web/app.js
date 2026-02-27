// IronClaw web UI — entry point (placeholder)
document.addEventListener('DOMContentLoaded', async () => {
    const app = document.getElementById('app');
    try {
        const res = await fetch('/api/status');
        const data = await res.json();
        app.innerHTML = `<p>Mode: ${data.mode}</p>`;
    } catch {
        app.innerHTML = '<p>IronClaw</p>';
    }
});
