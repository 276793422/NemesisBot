// Browser environment mock for testing browser JS in Node.js
// Uses vm.createContext for sandboxed execution with controlled globals

import vm from 'node:vm';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { JSDOM } from 'jsdom';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const DEFAULT_HTML = `<!DOCTYPE html><html><body>
  <div id="messages-container"></div>
  <div id="login-screen"></div>
  <div id="chat-screen" style="display:none"></div>
  <input id="message-input" type="text"/>
  <button id="send-button">\u53D1\u9001</button>
  <button id="login-button">\u767B\u5F55</button>
  <input id="auth-token-input" type="text"/>
  <input id="remember-me" type="checkbox"/>
  <div id="login-error"></div>
  <button id="logout-button">\u9000\u51FA</button>
  <span class="status-dot"></span>
  <span class="status-text"></span>
</body></html>`;

/**
 * Creates a sandboxed browser-like environment for testing.
 * Returns the context, captured sent messages, mock WebSocket class, and jsdom instance.
 */
export function createSandbox(options = {}) {
  const html = options.html || DEFAULT_HTML;
  const dom = new JSDOM(html, { url: 'http://localhost:8080' });

  const sentMessages = [];

  class MockWebSocket {
    static OPEN = 1;
    static CLOSED = 3;
    static CONNECTING = 0;
    static CLOSING = 2;

    readyState = MockWebSocket.OPEN;
    onopen = null;
    onmessage = null;
    onclose = null;
    onerror = null;

    constructor(url) {
      this.url = url;
    }

    send(data) {
      sentMessages.push(JSON.parse(data));
    }

    close(code, reason) {
      this.readyState = MockWebSocket.CLOSED;
      if (this.onclose) {
        this.onclose({ code: code || 1000, reason: reason || '' });
      }
    }
  }

  const globals = {
    console: { log: () => {}, error: () => {}, warn: () => {} },
    WebSocket: MockWebSocket,
    window: dom.window,
    document: dom.window.document,
    localStorage: dom.window.localStorage,
    setTimeout: options.setTimeout || setTimeout,
    setInterval: options.setInterval || setInterval,
    clearTimeout: options.clearTimeout || clearTimeout,
    clearInterval: options.clearInterval || clearInterval,
    JSON,
    Date,
    Math,
    Map,
    Set,
    Array,
    Object,
    String,
    Number,
    Boolean,
    Error,
    TypeError,
    encodeURIComponent,
    decodeURIComponent,
    parseInt,
    parseFloat,
    isNaN,
    isFinite,
    RegExp,
    fetch: () => Promise.resolve({ ok: true, json: () => Promise.resolve({}) }),
    EventSource: class {
      constructor() {}
      close() {}
      addEventListener() {}
    },
    alert: () => {},
    confirm: () => true,
    // Mock Alpine for chat.js
    Alpine: {
      store: () => ({ token: null, connected: false }),
    },
    // Mock highlight.js
    hljs: {
      highlight: (code) => ({ value: code }),
      highlightAuto: (code) => ({ value: code }),
      getLanguage: () => false,
      highlightElement: () => {},
    },
    // Mock marked
    marked: {
      setOptions: () => {},
      parse: (text) => text,
    },
  };

  const context = vm.createContext(globals);

  // Helper to evaluate JS code in the sandbox context
  function evalCode(code) {
    return vm.runInContext(code, context);
  }

  return { context, evalCode, sentMessages, MockWebSocket, dom, globals };
}

/**
 * Loads a source file into the sandbox context.
 * Use for files that declare globals with `var` (e.g. NemesisAPI).
 */
export function loadSource(context, filePath) {
  const source = fs.readFileSync(filePath, 'utf-8');
  vm.runInContext(source, context, { filename: path.basename(filePath) });
}

/**
 * Loads a source file and exports specified names to the context object.
 * Use for files that declare `class` (which doesn't become a global property).
 */
export function loadSourceWithExports(context, filePath, exports) {
  const source = fs.readFileSync(filePath, 'utf-8');
  const exportCode = exports
    .map((name) => `this.${name} = typeof ${name} !== 'undefined' ? ${name} : undefined;`)
    .join('\n');
  const wrappedSource = source + '\n\n' + exportCode;
  vm.runInContext(wrappedSource, context, { filename: path.basename(filePath) });
}

/**
 * Resolves a path relative to the project root.
 * test/js/ -> ../../ maps to the project root.
 */
export function projectPath(relativePath) {
  // __dirname is test/js/__mocks__/, need 3 levels up to project root
  return path.resolve(__dirname, '..', '..', '..', relativePath);
}
