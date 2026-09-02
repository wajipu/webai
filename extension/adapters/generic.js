// WebAI Bridge — heuristic fallback adapter.
//
// Works on any SPA chat that exposes a contenteditable/textarea composer and
// a visible send button. It is deliberately conservative:
//   * input via execCommand('insertText') (rich editors: ProseMirror, Lexical, Slate)
//   * send via the first enabled visible button matching sendCandidates
//   * completion via "stop indicator" + assistant message stabilization
//
// Concrete sites should override the selector table; only genuinely unknown
// pages should fall back to this.

const DEFAULT_SEL = {
  composer:
    'div[contenteditable="true"][role="textbox"], textarea[placeholder], div[contenteditable="true"]',
  sendButton:
    'button[type="submit"], button[aria-label*="send" i], button[aria-label*="Send" i], button[data-testid*="send" i]',
  stopButton:
    'button[aria-label*="stop" i], button[data-testid*="stop" i], [class*="stop" i]',
  userMsg:
    '[data-message-author-role="user"], [data-testid*="user-message"], [class*="user-message" i], [class*="user_message" i]',
  assistantMsg:
    '[data-message-author-role="assistant"], [data-testid*="assistant-message"], [class*="assistant-message" i], [class*="assistant_message" i], .markdown, [class*="message-content" i]',
  loginLink: 'a[href*="login"], a[href*="signin"], button[data-testid*="login" i]',
};

function buildAdapter(name, overrides = {}) {
  const sel = { ...DEFAULT_SEL, ...overrides };
  return {
    name,
    sel,
    isLoginWall() {
      const bodyText = document.body ? document.body.innerText : '';
      return (
        (bodyText.includes('Log in') || bodyText.includes('Sign up') || bodyText.includes('登录')) &&
        !!document.querySelector(sel.loginLink)
      );
    },
  };
}

window.WebAIAdapters = window.WebAIAdapters || {};
window.WebAIAdapters.generic = buildAdapter('generic');