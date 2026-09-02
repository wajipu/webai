// WebAI Bridge — background service worker.
//
// Responsibilities:
//   1. Keep a WebSocket connection to the local daemon (ws://127.0.0.1:8765).
//   2. On `ask`: find/create a chatgpt.com tab, wait until its content script
//      is ready, forward the ask, and relay the result back to the daemon.
//   3. Keep the service worker alive with chrome.alarms-driven pings.

const DEFAULT_WS_URL = 'ws://127.0.0.1:8765';

// site key -> { urls (tab match patterns), home (default tab to open) }
const SITES = {
  chatgpt: {
    urls: ['https://chatgpt.com/*', 'https://www.chatgpt.com/*', 'https://chat.openai.com/*'],
    home: 'https://chatgpt.com/',
  },
  grok: {
    urls: ['https://grok.com/*', 'https://www.grok.com/*', 'https://x.com/*', 'https://www.x.com/*'],
    home: 'https://grok.com/',
  },
  kimi: {
    urls: ['https://kimi.com/*', 'https://www.kimi.com/*', 'https://moonshot.cn/*', 'https://www.moonshot.cn/*'],
    home: 'https://kimi.com/',
  },
  glm: {
    urls: ['https://chatglm.cn/*', 'https://www.chatglm.cn/*', 'https://z.ai/*', 'https://www.z.ai/*'],
    home: 'https://chatglm.cn/',
  },
};

let ws = null;
let wsState = 'disconnected'; // disconnected | connecting | connected
let busy = false;
let retryDelay = 1000;

// ---------------------------------------------------------------- socket

async function getWsUrl() {
  const { wsUrl } = await chrome.storage.local.get('wsUrl');
  return wsUrl || DEFAULT_WS_URL;
}

function setStatus(status, detail = '') {
  wsState = status;
  const badge = status === 'connected' ? 'ON' : status === 'connecting' ? '…' : 'OFF';
  const color = status === 'connected' ? '#1a7f37' : '#b3261e';
  chrome.action.setBadgeText({ text: badge }).catch(() => {});
  chrome.action.setBadgeBackgroundColor({ color }).catch(() => {});
  chrome.runtime.sendMessage({ type: 'bridgeStatus', status, detail }).catch(() => {});
}

function connect() {
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return;
  setStatus('connecting');
  getWsUrl().then((url) => {
    ws = new WebSocket(url);
    ws.onopen = () => {
      retryDelay = 1000;
      setStatus('connected');
      ws.send(JSON.stringify({ type: 'hello', role: 'extension' }));
    };
    ws.onmessage = (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (msg.type === 'ask') {
        handleAsk(msg);
      } else if (msg.type === 'ping') {
        wsSend({ type: 'pong' });
      }
    };
    ws.onclose = () => {
      setStatus('disconnected');
      ws = null;
      busy = false;
      setTimeout(connect, retryDelay);
      retryDelay = Math.min(retryDelay * 2, 10000);
    };
    ws.onerror = () => {
      try {
        ws.close();
      } catch {}
    };
  });
}

function wsSend(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(obj));
  }
}

function reply(askId, result) {
  wsSend({ type: 'ask_result', id: askId, ...result });
}

// ---------------------------------------------------------------- keepalive

chrome.alarms.create('ping', { periodInMinutes: 0.25 });
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === 'ping') {
    if (ws && ws.readyState === WebSocket.OPEN) {
      wsSend({ type: 'ping' });
    } else {
      connect();
    }
  }
});

// reconnect when the daemon comes back later
setInterval(connect, 5000);

chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg && msg.type === 'keepalive') {
    // content script pings us while the page generates; receiving a message
    // resets the idle timer so the service worker is not reaped mid-ask
    sendResponse({ ok: true });
    return false;
  }
  if (msg && msg.type === 'getBridgeStatus') {
    sendResponse({ status: wsState, busy });
    return false;
  }
  if (msg && msg.type === 'wsUrlChanged') {
    if (ws) {
      try { ws.close(); } catch {}
    }
    connect();
  }
  return false;
});

// ---------------------------------------------------------------- ask flow

async function handleAsk(msg) {
  if (busy) {
    reply(msg.id, {
      ok: false,
      error: { code: 'BUSY', message: 'extension is already handling an ask' },
    });
    return;
  }
  busy = true;
  try {
    const result = await runAsk(msg.id, msg.payload);
    reply(msg.id, result);
  } catch (e) {
    reply(msg.id, {
      ok: false,
      error: { code: 'INTERNAL', message: String(e && e.message ? e.message : e) },
    });
  } finally {
    busy = false;
  }
}

async function runAsk(id, payload) {
  const timeoutMs = payload.timeout_ms || 300000;
  const site = payload.site || 'chatgpt';

  // hard cap: the whole ask flow must finish within timeoutMs + margin,
  // otherwise reply with TIMEOUT instead of hanging forever
  const result = await withTimeout(
    askOnTab(id, payload, site, timeoutMs),
    timeoutMs + 30000
  );
  if (result === null) {
    return { ok: false, error: { code: 'TIMEOUT', message: 'extension-side timeout' } };
  }
  return result;
}

async function askOnTab(id, payload, site, timeoutMs) {
  const tab = await ensureSiteTab(site, payload.conversation || null);

  // wait for the content script to be alive (tab may pre-date the extension)
  await waitContentScriptReady(tab.id);

  const resp = await chrome.tabs.sendMessage(tab.id, {
    type: 'ask',
    id,
    payload: {
      message: payload.message,
      conversation: payload.conversation || null,
      timeout_ms: timeoutMs,
    },
  });
  if (!resp) {
    throw new Error('no response from page — did the site change its DOM?');
  }
  if (resp.ok) {
    return { ok: true, data: resp.data };
  }
  return { ok: false, error: resp.error };
}

function withTimeout(promise, ms) {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(null), ms);
    promise
      .then((v) => {
        clearTimeout(timer);
        resolve(v);
      })
      .catch((e) => {
        clearTimeout(timer);
        resolve({ ok: false, error: { code: 'INTERNAL', message: String(e && e.message ? e.message : e) } });
      });
  });
}

async function ensureSiteTab(site, conversationUrl) {
  const cfg = SITES[site] || SITES.chatgpt;
  const tabs = await chrome.tabs.query({ url: cfg.urls });
  if (conversationUrl) {
    const want = new URL(conversationUrl);
    const match = tabs.find((t) => t.url && new URL(t.url).pathname === want.pathname);
    if (match) {
      await chrome.tabs.update(match.id, { active: true });
      return match;
    }
    const created = await chrome.tabs.create({ url: conversationUrl, active: true });
    await waitTabLoaded(created.id);
    return created;
  }
  if (tabs.length > 0) {
    await chrome.tabs.update(tabs[0].id, { active: true });
    return tabs[0];
  }
  const created = await chrome.tabs.create({ url: cfg.home, active: true });
  await waitTabLoaded(created.id);
  return created;
}

function waitTabLoaded(tabId) {
  // event-driven + bounded: setInterval polling can outlive the MV3 service
  // worker (it gets reaped after ~30s idle), which would strand the ask
  return new Promise((resolve) => {
    const started = Date.now();
    const MAX_WAIT = 25000;
    const finish = (tab) => {
      clearInterval(timer);
      resolve(tab);
    };
    const onUpdated = (id, info, tab) => {
      if (id === tabId && info.status === 'complete') finish(tab);
    };
    chrome.tabs.onUpdated.addListener(onUpdated);
    const timer = setInterval(() => {
      if (Date.now() - started > MAX_WAIT) {
        chrome.tabs.onUpdated.removeListener(onUpdated);
        finish(null); // probe loop downstream will handle the rest
        return;
      }
      chrome.tabs.get(tabId).then((t) => {
        if (t && t.status === 'complete') {
          chrome.tabs.onUpdated.removeListener(onUpdated);
          finish(t);
        }
      }).catch(() => {});
    }, 2000);
    chrome.tabs.get(tabId).then((t) => {
      if (t && t.status === 'complete') {
        chrome.tabs.onUpdated.removeListener(onUpdated);
        finish(t);
      }
    }).catch(() => {});
  });
}

async function waitContentScriptReady(tabId) {
  for (let i = 0; i < 20; i++) {
    try {
      const resp = await chrome.tabs.sendMessage(tabId, { type: 'probe' });
      if (resp && resp.ok) return;
    } catch {}
    await sleep(1000);
  }
  // content script present but dead? reload once and retry briefly
  try {
    await chrome.tabs.reload(tabId);
  } catch {}
  await sleep(3000);
  for (let i = 0; i < 15; i++) {
    try {
      const resp = await chrome.tabs.sendMessage(tabId, { type: 'probe' });
      if (resp && resp.ok) return;
    } catch {}
    await sleep(1000);
  }
  const t = await chrome.tabs.get(tabId).catch(() => null);
  throw new Error(
    `page is not reachable — ${t && t.url ? t.url : 'tab gone'}. ` +
      'Is the tab still open? Did the site redirect to a host the extension does not cover?'
  );
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// boot
chrome.runtime.onInstalled.addListener(connect);
connect();
