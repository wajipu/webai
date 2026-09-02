// WebAI Bridge — chatgpt.com adapter.
// Site contract selectors are at the top; when ChatGPT redesigns their DOM,
// only this file should need changes.

const SEL = {
  composer:
    '#prompt-textarea[contenteditable="true"], div[contenteditable="true"]#prompt-textarea, form textarea#prompt-textarea',
  sendButton: '[data-testid="send-button"]',
  stopButton:
    '[data-testid="stop-button"], button[aria-label*="Stop generating"], button[data-testid="pill-stop-generating"]',
  userMsg: '[data-message-author-role="user"]',
  assistantMsg: '[data-message-author-role="assistant"]',
  loginLink: 'a[href*="/auth"], button[data-testid="login-button"]',
};

const Adapter = {
  name: 'chatgpt',
  sel: SEL,
  isLoginWall() {
    const bodyText = document.body ? document.body.innerText : '';
    return (
      bodyText.includes('Log in') ||
      bodyText.includes('Sign up') ||
      document.querySelector(SEL.loginLink)
    );
  },
};