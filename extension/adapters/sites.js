// WebAI Bridge — site-specific adapter overrides.
//
// ⚠️ These selectors are FIRST-GUESS values, not verified against the live
// sites yet. To calibrate: open the site, log in, right-click the composer
// → Inspect, and fix the entries below. The generic adapter will otherwise
// do its best heuristically (it often works for simple layouts).

window.WebAIAdapters = window.WebAIAdapters || {};

// grok.com — xAI chat
window.WebAIAdapters.grok = window.WebAIAdapters.generic && {
  ...window.WebAIAdapters.generic,
  name: 'grok',
  sel: {
    ...window.WebAIAdapters.generic.sel,
    // TODO(verify): composer/send/assistant selectors on grok.com
    composer: 'textarea, div[contenteditable="true"]',
  },
};

// kimi.com — Moonshot AI chat (React, Lexical-like rich editor)
window.WebAIAdapters.kimi = window.WebAIAdapters.generic && {
  ...window.WebAIAdapters.generic,
  name: 'kimi',
  sel: {
    ...window.WebAIAdapters.generic.sel,
    // TODO(verify): kimi uses a rich text composer; send button is often the last icon in the toolbar
    composer: 'div[contenteditable="true"][data-testid*="editor" i], div[contenteditable="true"], textarea',
  },
};

// chatglm.cn — Zhipu GLM chat (domestic)
window.WebAIAdapters.glm = window.WebAIAdapters.generic && {
  ...window.WebAIAdapters.generic,
  name: 'glm',
  sel: {
    ...window.WebAIAdapters.generic.sel,
    // TODO(verify): GLM's composer
    composer: 'textarea, div[contenteditable="true"]',
  },
};