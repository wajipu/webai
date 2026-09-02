// WebAI Bridge — chatgpt.com adapter (content script, isolated world).
//
// Site contract (all brittle selectors live here so the rest of the system
// only deals with the protocol):
//   composer   : the ProseMirror contenteditable at the bottom of the page
//   send       : the submit button
//   generating : stop button / streaming marker presence
//   assistant  : latest assistant turn element
//
// When ChatGPT redesigns their DOM, only this file should need changes.

const SEL = {
  composer: '#prompt-textarea[contenteditable="true"], div[contenteditable="true"]#prompt-textarea, form textarea#prompt-textarea',
  sendButton: '[data-testid="send-button"]',
  stopButton: '[data-testid="stop-button"], button[aria-label*="Stop generating"], button[data-testid="pill-stop-generating"]',
  userMsg: '[data-message-author-role="user"]',
  assistantMsg: '[data-message-author-role="assistant"]',
  loginLink: 'a[href*="/auth"], button[data-testid="login-button"]',
};

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
    sendResponse({ ok: true, onChatGpt: true });
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

  const composer = await waitFor(() => {
    const el = $(SEL.composer);
    return visible(el) ? el : null;
  }, 25000);

  if (!composer) {
    // maybe a login wall
    const bodyText = document.body ? document.body.innerText : '';
    if (bodyText.includes('Log in') || bodyText.includes('Sign up') || $(SEL.loginLink)) {
      return {
        ok: false,
        error: {
          code: 'LOGIN_REQUIRED',
          message: 'chatgpt.com is not logged in — open the tab and sign in once',
        },
      };
    }
    return {
      ok: false,
      error: { code: 'SITE_DRIFT', message: 'composer not found — ChatGPT UI may have changed' },
    };
  }

  // ---- type the message (ProseMirror accepts execCommand('insertText'),
  // which fires real beforeinput/input events)
  composer.focus();
  document.execCommand('insertText', false, message);

  // ---- send
  const sent = await waitFor(async () => {
    const btn = $(SEL.sendButton);
    if (btn && visible(btn) && !btn.disabled) {
      btn.click();
      return true;
    }
    // if there is no dedicated send button, try Enter on the composer
    return null;
  }, 15000, 400);

  if (!sent) {
    // composer may have auto-sent via keyboard already; verify it cleared
    await sleep(2000);
    if (composer.innerText.trim().length > 0) {
      return { ok: false, error: { code: 'SITE_DRIFT', message: 'send button not found' } };
    }
  }

  // wait until the composer actually cleared (message accepted)
  await waitFor(() => composer.innerText.trim().length === 0, 20000, 500).catch(() => {});

  // ---- wait for generation to finish
  const deadline = Date.now() + timeoutMs;
  let lastText = '';
  let stableRounds = 0;

  while (Date.now() < deadline) {
    const generating = !!$(SEL.stopButton) || !!document.querySelector('.result-streaming');
    const assistants = document.querySelectorAll(SEL.assistantMsg);
    const last = assistants.length ? assistants[assistants.length - 1] : null;
    const text = last ? last.innerText.trim() : '';

    if (!generating && text.length > 0) {
      if (text === lastText) {
        stableRounds++;
        if (stableRounds >= 3) {
          const data = {
            text,
            url: location.href,
            title: document.title,
          };
          return { ok: true, data };
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
      message: `no stable reply within ${Math.round(timeoutMs / 1000)}s`,
    },
  };
}
