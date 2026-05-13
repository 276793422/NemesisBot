// Message Handler Routing Tests
// Verifies that UIController.handleMessage() and chatPage().onMessage()
// correctly route incoming messages by type/module/cmd.

import { describe, it, expect, beforeEach } from 'vitest';
import {
  createSandbox,
  loadSourceWithExports,
  loadSource,
  projectPath,
} from '../__mocks__/browser.js';

const APP_SOURCE = 'module/web/static/chat/app.js';
const API_SOURCE = 'module/web/static/js/api.js';
const CHAT_SOURCE = 'module/web/static/js/pages/chat.js';
const APP_EXPORTS = ['AuthManager', 'WebSocketManager', 'MessageRenderer', 'UIController'];

// ============================================================
// UIController.handleMessage() Tests
// ============================================================

describe('UIController Message Routing', () => {
  let sandbox;

  beforeEach(() => {
    sandbox = createSandbox();
    loadSourceWithExports(sandbox.context, projectPath(APP_SOURCE), APP_EXPORTS);
  });

  function createController(name) {
    sandbox.evalCode(`
      var ${name} = new UIController();
      // Mock renderer to capture calls
      ${name}.renderer = {
        _lastAppend: undefined,
        _lastPrepend: undefined,
        appendMessage: function(role, content, timestamp, isError, isSystem) {
          this._lastAppend = { role: role, content: content, timestamp: timestamp, isError: !!isError, isSystem: !!isSystem };
        },
        prependMessages: function(msgs) {
          this._lastPrepend = msgs;
        },
        clear: function() {}
      };
      // Mock wsManager
      ${name}.wsManager = { sendHistoryRequest: function() {} };
      // Mock input elements
      ${name}.input = { disabled: false, value: '', focus: function(){}, style: {} };
      ${name}.sendButton = { disabled: false, textContent: '\u53D1\u9001' };
      // Mock status elements
      ${name}.statusIndicator = { classList: { remove: function(){}, add: function(){} } };
      ${name}.statusText = { textContent: '' };
    `);
  }

  // --- message.chat.receive ---

  describe('message.chat.receive', () => {
    it('should route to renderer.appendMessage', () => {
      createController('_ctrl1');
      sandbox.evalCode(`
        _ctrl1.handleMessage({
          type: 'message',
          module: 'chat',
          cmd: 'receive',
          data: { role: 'assistant', content: 'Hello!' },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const result = sandbox.evalCode(`_ctrl1.renderer._lastAppend`);
      expect(result.role).toBe('assistant');
      expect(result.content).toBe('Hello!');
      expect(result.isError).toBe(false);
    });

    it('should use default role when missing', () => {
      createController('_ctrl2');
      sandbox.evalCode(`
        _ctrl2.handleMessage({
          type: 'message',
          module: 'chat',
          cmd: 'receive',
          data: { content: 'No role field' },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const result = sandbox.evalCode(`_ctrl2.renderer._lastAppend`);
      expect(result.role).toBe('assistant'); // default fallback
      expect(result.content).toBe('No role field');
    });

    it('should re-enable input after receiving', () => {
      createController('_ctrl3');
      sandbox.evalCode(`
        _ctrl3.input.disabled = true;
        _ctrl3.sendButton.disabled = true;
        _ctrl3.handleMessage({
          type: 'message',
          module: 'chat',
          cmd: 'receive',
          data: { role: 'assistant', content: 'done' },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const inputDisabled = sandbox.evalCode(`_ctrl3.input.disabled`);
      const btnDisabled = sandbox.evalCode(`_ctrl3.sendButton.disabled`);
      expect(inputDisabled).toBe(false);
      expect(btnDisabled).toBe(false);
    });
  });

  // --- message.chat.history ---

  describe('message.chat.history', () => {
    it('should route to handleHistoryResponse', () => {
      createController('_ctrl4');
      sandbox.evalCode(`
        _ctrl4._historyLoading = true;
        _ctrl4.handleMessage({
          type: 'message',
          module: 'chat',
          cmd: 'history',
          data: {
            messages: [{ role: 'user', content: 'hi' }, { role: 'assistant', content: 'hello' }],
            has_more: true,
            oldest_index: 5,
            total_count: 20
          },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const loading = sandbox.evalCode(`_ctrl4._historyLoading`);
      expect(loading).toBe(false);
      const hasMore = sandbox.evalCode(`_ctrl4._hasMoreHistory`);
      expect(hasMore).toBe(true);
      const oldestIndex = sandbox.evalCode(`_ctrl4._oldestIndex`);
      expect(oldestIndex).toBe(5);
      const prepend = sandbox.evalCode(`_ctrl4.renderer._lastPrepend`);
      expect(prepend).toHaveLength(2);
    });

    it('should set hasMore false when oldest_index is 0', () => {
      createController('_ctrl5');
      sandbox.evalCode(`
        _ctrl5._historyLoading = true;
        _ctrl5.handleMessage({
          type: 'message',
          module: 'chat',
          cmd: 'history',
          data: {
            messages: [{ role: 'user', content: 'first' }],
            has_more: false,
            oldest_index: 0,
            total_count: 1
          }
        });
      `);

      const hasMore = sandbox.evalCode(`_ctrl5._hasMoreHistory`);
      expect(hasMore).toBe(false);
    });
  });

  // --- system.heartbeat.pong ---

  describe('system.heartbeat.pong', () => {
    it('should be silently ignored', () => {
      createController('_ctrl6');
      sandbox.evalCode(`
        _ctrl6.handleMessage({
          type: 'system',
          module: 'heartbeat',
          cmd: 'pong',
          data: {},
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const result = sandbox.evalCode(`_ctrl6.renderer._lastAppend`);
      expect(result).toBeUndefined();
    });
  });

  // --- system.error.notify ---

  describe('system.error.notify', () => {
    it('should route to renderer as error message', () => {
      createController('_ctrl7');
      sandbox.evalCode(`
        _ctrl7.handleMessage({
          type: 'system',
          module: 'error',
          cmd: 'notify',
          data: { content: 'Something went wrong' },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const result = sandbox.evalCode(`_ctrl7.renderer._lastAppend`);
      expect(result.content).toBe('Something went wrong');
      expect(result.isError).toBe(true);
    });
  });

  // --- unknown messages ---

  describe('unknown message types', () => {
    it('should ignore unknown type/module/cmd combinations', () => {
      createController('_ctrl8');
      // Should not throw
      sandbox.evalCode(`
        _ctrl8.handleMessage({
          type: 'unknown',
          module: 'test',
          cmd: 'test',
          data: {}
        });
      `);

      const result = sandbox.evalCode(`_ctrl8.renderer._lastAppend`);
      expect(result).toBeUndefined();
    });
  });
});

// ============================================================
// UIController.handleHistoryResponse() Tests
// ============================================================

describe('UIController handleHistoryResponse', () => {
  let sandbox;

  beforeEach(() => {
    sandbox = createSandbox();
    loadSourceWithExports(sandbox.context, projectPath(APP_SOURCE), APP_EXPORTS);
  });

  function createController(name) {
    sandbox.evalCode(`
      var ${name} = new UIController();
      ${name}.renderer = {
        _lastPrepend: undefined,
        prependMessages: function(msgs) { this._lastPrepend = msgs; },
      };
    `);
  }

  it('should handle null data gracefully', () => {
    createController('_hr1');
    sandbox.evalCode(`
      _hr1._historyLoading = true;
      _hr1.handleHistoryResponse(null);
    `);

    const loading = sandbox.evalCode(`_hr1._historyLoading`);
    expect(loading).toBe(false);
  });

  it('should handle data with empty messages', () => {
    createController('_hr2');
    sandbox.evalCode(`
      _hr2._historyLoading = true;
      _hr2.handleHistoryResponse({ messages: [], has_more: false, oldest_index: 0, total_count: 0 });
    `);

    const loading = sandbox.evalCode(`_hr2._historyLoading`);
    expect(loading).toBe(false);
    const prepend = sandbox.evalCode(`_hr2.renderer._lastPrepend`);
    expect(prepend).toBeUndefined(); // no prepend for empty array
  });

  it('should track oldest_index and has_more', () => {
    createController('_hr3');
    sandbox.evalCode(`
      _hr3._historyLoading = true;
      _hr3.handleHistoryResponse({
        messages: [{ role: 'user', content: 'hi' }],
        has_more: true,
        oldest_index: 10,
        total_count: 50
      });
    `);

    expect(sandbox.evalCode(`_hr3._oldestIndex`)).toBe(10);
    expect(sandbox.evalCode(`_hr3._hasMoreHistory`)).toBe(true);
  });
});

// ============================================================
// chatPage().onMessage() Tests
// ============================================================

describe('chatPage onMessage Routing', () => {
  let sandbox;

  beforeEach(() => {
    sandbox = createSandbox();
    // Load NemesisAPI first (chatPage references it)
    loadSource(sandbox.context, projectPath(API_SOURCE));
    // Load chatPage function
    loadSource(sandbox.context, projectPath(CHAT_SOURCE));
  });

  function createChatPage(name) {
    sandbox.evalCode(`
      var ${name} = chatPage();
      ${name}.messages = [];
      ${name}.streaming = false;
      ${name}.$refs = {
        chatMessages: { scrollTop: 0, scrollHeight: 100 },
        chatInput: { style: {} }
      };
      ${name}.$nextTick = function(cb) { cb(); };
      ${name}.$el = { querySelectorAll: function() { return []; } };
    `);
  }

  // --- message.chat.receive ---

  describe('message.chat.receive', () => {
    it('should push message to array and stop streaming', () => {
      createChatPage('_cp1');
      sandbox.evalCode(`
        _cp1.streaming = true;
        _cp1.onMessage({
          type: 'message',
          module: 'chat',
          cmd: 'receive',
          data: { role: 'assistant', content: 'Hi there!' },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const msgs = sandbox.evalCode(`_cp1.messages`);
      expect(msgs).toHaveLength(1);
      expect(msgs[0].role).toBe('assistant');
      expect(msgs[0].content).toBe('Hi there!');
      expect(msgs[0].timestamp).toBe('2026-04-18T12:00:00Z');
      expect(sandbox.evalCode(`_cp1.streaming`)).toBe(false);
    });

    it('should use default role when missing', () => {
      createChatPage('_cp2');
      sandbox.evalCode(`
        _cp2.onMessage({
          type: 'message',
          module: 'chat',
          cmd: 'receive',
          data: { content: 'No role' },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const msgs = sandbox.evalCode(`_cp2.messages`);
      expect(msgs[0].role).toBe('assistant');
    });
  });

  // --- system.error.notify ---

  describe('system.error.notify', () => {
    it('should push error message and stop streaming', () => {
      createChatPage('_cp3');
      sandbox.evalCode(`
        _cp3.streaming = true;
        _cp3.onMessage({
          type: 'system',
          module: 'error',
          cmd: 'notify',
          data: { content: 'Error occurred' },
          timestamp: '2026-04-18T12:00:00Z'
        });
      `);

      const msgs = sandbox.evalCode(`_cp3.messages`);
      expect(msgs).toHaveLength(1);
      expect(msgs[0].role).toBe('error');
      expect(msgs[0].content).toBe('Error occurred');
      expect(sandbox.evalCode(`_cp3.streaming`)).toBe(false);
    });
  });

  // --- system.heartbeat.pong ---

  describe('system.heartbeat.pong', () => {
    it('should be silently ignored', () => {
      createChatPage('_cp4');
      sandbox.evalCode(`
        _cp4.onMessage({
          type: 'system',
          module: 'heartbeat',
          cmd: 'pong',
          data: {}
        });
      `);

      const msgs = sandbox.evalCode(`_cp4.messages`);
      expect(msgs).toHaveLength(0);
    });
  });

  // --- old format (no module field) ---

  describe('old format without module', () => {
    it('should be ignored (Phase 3 removed old format support)', () => {
      createChatPage('_cp5');
      sandbox.evalCode(`
        _cp5.onMessage({
          type: 'message',
          content: 'old format message'
        });
      `);

      const msgs = sandbox.evalCode(`_cp5.messages`);
      expect(msgs).toHaveLength(0);
    });
  });

  // --- unknown protocol messages ---

  describe('unknown protocol messages', () => {
    it('should ignore unknown type/module combinations', () => {
      createChatPage('_cp6');
      sandbox.evalCode(`
        _cp6.onMessage({
          type: 'agent',
          module: 'status',
          cmd: 'update',
          data: {}
        });
      `);

      const msgs = sandbox.evalCode(`_cp6.messages`);
      expect(msgs).toHaveLength(0);
    });
  });
});
