// WebAI Bridge — content script dispatcher + shared engine.
//
// Loaded on every matched host. Picks the right adapter by hostname
// (chatgpt.com / grok.com / kimi.com / chatglm.cn / generic fallback) and
// drives the conversation with the shared engine below.
//
// The engine is deliberately site-agnostic: it only uses what the adapter
// declares (selectors + isLoginWall()).

const ADAPTERS = {
  'chatgpt.com': 'chatgpt',
  'chat.openai.com': 'chatgpt',
  'grok.com': 'grok',
  'x.com': 'grok',
  'kimi.com': 'kimi',
  'moonshot.cn': 'kimi',
  'chatglm.cn': 'glm',
  'z.ai': 'glm',
};

function currentAdapter() {
  const host = location.hostname.replace(/^www\./, '');
  const key = ADAPTERS[host] || 'generic';
  const adapters = window.WebAIAdapters || {};
  if (adapters[key]) return adapters[key];
  return adapters.generic;
}

function $(sel) {
  return document.querySelector(sel);
}

function visible(el) {
  if (!el) return false;
  const r = el.getBoundingClientRect();
  return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== 'hidden';
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// textarea holds its content in .value, contenteditable in innerText
function composerText(el) {
  if (!el) return '';
  if (el.value !== undefined) return String(el.value);
  return el.innerText || '';
}

async function waitFor(fn, timeoutMs, intervalMs = 500) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const v = fn();
    if (v) return v;
    await sleep(intervalMs);
  }
  return null;
}

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (!msg) return false;
  if (msg.type === 'probe') {
    sendResponse({ ok: true, onWebAI: true });
    return false;
  }
  if (msg.type === 'ask') {
    handleAsk(msg)
      .then((result) => sendResponse(result))
      .catch((e) =>
        sendResponse({ ok: false, error: { code: 'INTERNAL', message: String(e) } })
      );
    return true; // async response
  }
  return false;
});

async function handleAsk(msg) {
  const { message, timeout_ms: timeoutMs = 300000 } = msg.payload;
  const adapter = currentAdapter();
  const sel = adapter.sel;

  const composer = await waitFor(() => {
    const el = $(sel.composer);
    return visible(el) ? el : null;
  }, 25000);

  if (!composer) {
    if (adapter.isLoginWall && adapter.isLoginWall()) {
      return {
        ok: false,
        error: {
          code: 'LOGIN_REQUIRED',
          message: `not logged in on ${location.hostname} — open the tab and sign in once`,
        },
      };
    }
    return {
      ok: false,
      error: {
        code: 'SITE_DRIFT',
        message: `composer not found on ${location.hostname} (${adapter.name} adapter)`,
      },
    };
  }

  // ---- type the message (rich editors accept execCommand('insertText'),
  // which fires real beforeinput/input events)
  composer.focus();
  document.execCommand('insertText', false, message);
  // belt & braces: for plain <textarea> composers (Vue/React controlled),
  // set .value directly and fire input/change so the framework updates its
  // state (e.g. removes a disabled `empty` class from the send button)
  if (composer.value !== undefined) {
    composer.value = message;
    composer.dispatchEvent(new Event('input', { bubbles: true }));
    composer.dispatchEvent(new Event('change', { bubbles: true }));
  } else {
    composer.dispatchEvent(new Event('input', { bubbles: true }));
  }

  // ---- send (prefer a dedicated send button; fall back to Enter)
  const sent = await waitFor(() => {
    const btn = $(sel.sendButton);
    if (btn && visible(btn) && !btn.disabled) {
      btn.click();
      return true;
    }
    return null;
  }, 15000, 400);

  if (!sent) {
    await sleep(2000);
    if (composerText(composer).length > 0) {
      return {
        ok: false,
        error: {
          code: 'SITE_DRIFT',
          message: 'send button not found; composer did not clear either',
        },
      };
    }
  }

  // wait until the composer actually cleared (message accepted)
  await waitFor(() => composerText(composer).length === 0, 20000, 500).catch(() => {});

  // ---- wait for generation to finish
  const deadline = Date.now() + timeoutMs;
  let lastText = '';
  let stableRounds = 0;

  while (Date.now() < deadline) {
    const generating = !!$(sel.stopButton) || !!document.querySelector('.result-streaming');
    const nodes = document.querySelectorAll(sel.assistantMsg);
    const last = nodes.length ? nodes[nodes.length - 1] : null;
    const text = last ? last.innerText.trim() : '';

    if (!generating && text.length > 0) {
      if (text === lastText) {
        stableRounds++;
        if (stableRounds >= 3) {
          return {
            ok: true,
            data: {
              text,
              url: location.href,
              title: document.title,
              site: adapter.name,
            },
          };
        }
      } else {
        lastText = text;
        stableRounds = 1;
      }
    } else {
      stableRounds = 0;
      lastText = text;
    }
    await sleep(1500);
  }

  return {
    ok: false,
    error: {
      code: 'TIMEOUT',
      message: `no stable reply within ${Math.round(timeoutMs / 1000)}s (${adapter.name})`,
    },
  };
}