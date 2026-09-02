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

// kimi.com — Moonshot AI chat (Vue SPA)
window.WebAIAdapters.kimi = window.WebAIAdapters.generic && {
  ...window.WebAIAdapters.generic,
  name: 'kimi',
  sel: {
    ...window.WebAIAdapters.generic.sel,
    // verified: send button is a div.send-button-container wrapping an svg icon
    sendButton: 'div.send-button-container, button[type="submit"]',
  },
};

// chatglm.cn — Zhipu GLM chat (Vue SPA, domestic)
window.WebAIAdapters.glm = window.WebAIAdapters.generic && {
  ...window.WebAIAdapters.generic,
  name: 'glm',
  sel: {
    ...window.WebAIAdapters.generic.sel,
    // verified from live DOM:
    //   composer: textarea.scroll-display-none inside #search-input-box
    //   send: .enter-icon-container carries an `empty` class while the
    //         input is empty (disabled state) — exclude it
    composer: '#search-input-box textarea, textarea.scroll-display-none',
    sendButton: '.enter-icon-container:not(.empty), .enter.is-main-chat:not(.empty) img.enter_icon',
  },
};