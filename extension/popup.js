const dot = document.getElementById('dot');
const statusEl = document.getElementById('status');
const logEl = document.getElementById('log');
const input = document.getElementById('wsUrl');

async function refresh() {
  const { wsUrl } = await chrome.storage.local.get('wsUrl');
  input.value = wsUrl || 'ws://127.0.0.1:8765';
  const resp = await chrome.runtime.sendMessage({ type: 'getBridgeStatus' });
  if (!resp) return;
  const { status, busy } = resp;
  dot.className = 'dot' + (status === 'connected' ? ' on' : busy ? ' busy' : '');
  statusEl.textContent = status === 'connected' ? (busy ? 'connected · busy' : 'connected') : busy ? 'busy' : status || 'disconnected';
}

document.getElementById('save').addEventListener('click', async () => {
  const url = input.value.trim();
  if (!/^wss?:\/\/.+/.test(url)) {
    logEl.textContent = 'invalid URL (expects ws://127.0.0.1:8765)';
    return;
  }
  await chrome.storage.local.set({ wsUrl: url });
  await chrome.runtime.sendMessage({ type: 'wsUrlChanged' });
  logEl.textContent = 'saved, reconnecting…';
  setTimeout(refresh, 600);
});

chrome.runtime.onMessage.addListener((msg) => {
  if (msg && msg.type === 'bridgeStatus') {
    refresh();
  }
});

refresh();
