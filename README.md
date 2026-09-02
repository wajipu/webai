# webai — 命令行驱动网页版免费 AI

```
你 (终端) ── webai ask "…" ──► Rust daemon (ws://127.0.0.1:8765) ──► Chrome 扩展 ──► chatgpt.com(已登录)
你 (终端) ◄─ 结构化回复 ──── Rust daemon ◄─────────────────────────── 提取完成的消息
```

Rust 本地守护进程 + MV3 Chrome 扩展 + CLI。CLI 发消息,扩展把你**已登录**的
chatgpt.com 当执行引擎,等它生成完把回复取回终端。

## 下载即用(GitHub Releases)

打一个 tag 就自动出四个平台二进制 + 扩展 zip:

```bash
# 1. 下载二进制(Release 页面或一键脚本)
curl -fsSL https://raw.githubusercontent.com/wajipu/webai/main/install.sh | bash
export PATH="$HOME/.webai/bin:$PATH"

# 2. 起 daemon
webai serve

# 3. 解压 webai-extension.zip,chrome://extensions → 开发者模式 → 加载已解压的扩展程序
#    (扩展图标应变 ON)

# 4. 提问
webai ask "用一句话解释什么是 Rust 的所有权"
```

其他平台:Release 资产里有 `webai-darwin-arm64`、`webai-darwin-x64`、`webai-linux-x64`、`webai-win-x64.exe`。

## 组件

| 目录 | 说明 |
|---|---|
| `src/` | Rust:`webai serve`(WebSocket daemon + 请求路由)、`webai ask`(CLI)、`webai status` |
| `extension/` | MV3 扩展:background(WS 连接/心跳/找标签页)+ content script(chatgpt.com 适配器) |

## 线协议(JSON over WebSocket)

```
→ {type:"hello", role:"cli"|"extension"}
→ {type:"ask", id:"uuid", payload:{message, conversation?, timeout_ms?}}
← {type:"ask_result", id, ok:true, data:{text, url, title}}
← {type:"ask_result", id, ok:false, error:{code, message}}
```

错误码:`NO_EXTENSION` `BUSY` `NO_TAB` `LOGIN_REQUIRED` `SITE_DRIFT` `TIMEOUT`

## 运行

```bash
# 1. 启动 daemon
cargo run -- serve

# 2. 加载扩展
#    chrome://extensions → 开发者模式 → 加载已解压的扩展程序 → 选 extension/
#    扩展图标应显示 ON(连接 127.0.0.1:8765,可在 popup 改地址)

# 3. 提问
cargo run -- ask "用一句话解释什么是 Rust 的所有权"

# 结构化输出 / 多轮对话 / 状态
cargo run -- ask --json --conversation https://chatgpt.com/c/xxxx "继续说"
cargo run -- status
```

第一次跑:扩展会打开一个 chatgpt.com 标签页,**先在浏览器里登录一次**。

## 架构决策

- **扩展主动外连 daemon**(不监听端口),所以 MV3 也能用;心跳走 `chrome.alarms`,防 service worker 休眠断连
- 输入用 `document.execCommand('insertText')`:ChatGPT 的 ProseMirror 只认浏览器原生输入事件,直接赋值无效
- 完成判定 = 停止生成按钮消失 + 最新助手消息文本连续 3 轮不变(防"只发半句"的假完成)
- 所有站点 DOM 选择器集中在 `extension/content.js` 顶部 `SEL`,ChatGPT 改版只动这一个文件

## 安全

- daemon 只绑 `127.0.0.1`;后续可加 token 校验(握手 `hello` 携带)
- 扩展只注入 chatgpt.com / chat.openai.com
- 每次新对话默认不带上下文污染;`--conversation <url>` 可续旧会话

## Roadmap

- [x] P0:单次 ask,新对话
- [ ] P1:多轮 `--conversation` 深度测试、busy/恢复、daemon→cli 状态推送
- [ ] P2:Gemini / Grok / Claude.ai 适配器(`content/gemini.js` 之类,扩展按域名路由)
- [ ] P3:认证 token、审计日志
