// WebSocketManager Protocol Format Tests
// Verifies that the chat page WebSocketManager sends messages in the correct
// three-level dispatch protocol format (type -> module -> cmd).

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createSandbox, loadSourceWithExports, projectPath } from '../__mocks__/browser.js';

const SOURCE_FILE = 'module/web/static/chat/app.js';
const EXPORTS = ['AuthManager', 'WebSocketManager', 'MessageRenderer', 'UIController'];

describe('WebSocketManager Protocol Format', () => {
  let sandbox;

  beforeEach(() => {
    sandbox = createSandbox();
    loadSourceWithExports(sandbox.context, projectPath(SOURCE_FILE), EXPORTS);
  });

  function createManager(name) {
    sandbox.evalCode(`
      var ${name} = new WebSocketManager('ws://localhost:8080/ws', 'test-token');
      ${name}.ws = new WebSocket('ws://localhost:8080/ws');
      ${name}.ws.readyState = WebSocket.OPEN;
    `);
  }

  // --- send() ---

  describe('send()', () => {
    it('should send message in three-level protocol format', () => {
      createManager('_mgr1');
      sandbox.evalCode(`_mgr1.send('hello world')`);

      expect(sandbox.sentMessages).toHaveLength(1);
      const msg = sandbox.sentMessages[0];
      expect(msg.type).toBe('message');
      expect(msg.module).toBe('chat');
      expect(msg.cmd).toBe('send');
      expect(msg.data).toEqual({ content: 'hello world' });
      expect(msg.timestamp).toMatch(/^\d{4}-\d{2}-\d{2}T/);
    });

    it('should send message with special characters', () => {
      createManager('_mgr_special');
      sandbox.evalCode(`_mgr_special.send('hello "world" & <friends>')`);

      const msg = sandbox.sentMessages[0];
      expect(msg.data.content).toBe('hello "world" & <friends>');
    });

    it('should queue message when WebSocket is not open', () => {
      sandbox.evalCode(`
        var _mgr2 = new WebSocketManager('ws://localhost/ws', 'token');
        _mgr2.ws = { readyState: WebSocket.CLOSED };
      `);
      sandbox.evalCode(`_mgr2.send('queued msg')`);

      expect(sandbox.sentMessages).toHaveLength(0);
      const queue = sandbox.evalCode(`_mgr2.messageQueue`);
      expect(queue).toEqual(['queued msg']);
    });

    it('should queue message when WebSocket is null', () => {
      sandbox.evalCode(`
        var _mgr_null = new WebSocketManager('ws://localhost/ws', 'token');
        _mgr_null.ws = null;
      `);
      sandbox.evalCode(`_mgr_null.send('null ws')`);

      expect(sandbox.sentMessages).toHaveLength(0);
      const queue = sandbox.evalCode(`_mgr_null.messageQueue`);
      expect(queue).toEqual(['null ws']);
    });
  });

  // --- sendHistoryRequest() ---

  describe('sendHistoryRequest()', () => {
    it('should send history request without before_index', () => {
      createManager('_hMgr1');
      sandbox.evalCode(`_hMgr1.sendHistoryRequest('req-001', 20, null)`);

      expect(sandbox.sentMessages).toHaveLength(1);
      const msg = sandbox.sentMessages[0];
      expect(msg.type).toBe('message');
      expect(msg.module).toBe('chat');
      expect(msg.cmd).toBe('history_request');
      expect(msg.data.request_id).toBe('req-001');
      expect(msg.data.limit).toBe(20);
      expect(msg.data.before_index).toBeUndefined();
      expect(msg.timestamp).toBeTruthy();
    });

    it('should send history request with before_index', () => {
      createManager('_hMgr2');
      sandbox.evalCode(`_hMgr2.sendHistoryRequest('req-002', 10, 50)`);

      const msg = sandbox.sentMessages[0];
      expect(msg.data.request_id).toBe('req-002');
      expect(msg.data.limit).toBe(10);
      expect(msg.data.before_index).toBe(50);
    });

    it('should send history request with before_index = 0', () => {
      createManager('_hMgr3');
      sandbox.evalCode(`_hMgr3.sendHistoryRequest('req-003', 5, 0)`);

      const msg = sandbox.sentMessages[0];
      // 0 is falsy but should still be included (the check is !== null && !== undefined)
      expect(msg.data.before_index).toBe(0);
    });

    it('should not send when WebSocket is null', () => {
      sandbox.evalCode(`
        var _hMgr4 = new WebSocketManager('ws://localhost/ws', 'token');
        _hMgr4.ws = null;
      `);
      sandbox.evalCode(`_hMgr4.sendHistoryRequest('req-004', 20, null)`);
      expect(sandbox.sentMessages).toHaveLength(0);
    });
  });

  // --- startHeartbeat() ---

  describe('startHeartbeat()', () => {
    it('should send heartbeat ping in correct protocol format', () => {
      vi.useFakeTimers();

      const hbSandbox = createSandbox({
        setTimeout,
        setInterval,
        clearTimeout,
        clearInterval,
      });
      loadSourceWithExports(hbSandbox.context, projectPath(SOURCE_FILE), ['WebSocketManager']);

      hbSandbox.evalCode(`
        var _hbMgr = new WebSocketManager('ws://localhost/ws', 'token');
        _hbMgr.ws = new WebSocket('ws://localhost/ws');
        _hbMgr.ws.readyState = WebSocket.OPEN;
        _hbMgr.startHeartbeat();
      `);

      vi.advanceTimersByTime(30000);

      expect(hbSandbox.sentMessages.length).toBeGreaterThanOrEqual(1);
      const msg = hbSandbox.sentMessages[0];
      expect(msg.type).toBe('system');
      expect(msg.module).toBe('heartbeat');
      expect(msg.cmd).toBe('ping');
      expect(msg.data).toEqual({});
      expect(msg.timestamp).toBeTruthy();

      vi.useRealTimers();
    });

    it('should not send heartbeat when WebSocket is closed', () => {
      vi.useFakeTimers();

      const hbSandbox2 = createSandbox({
        setTimeout,
        setInterval,
        clearTimeout,
        clearInterval,
      });
      loadSourceWithExports(hbSandbox2.context, projectPath(SOURCE_FILE), ['WebSocketManager']);

      hbSandbox2.evalCode(`
        var _hbMgr2 = new WebSocketManager('ws://localhost/ws', 'token');
        _hbMgr2.ws = { readyState: WebSocket.CLOSED };
        _hbMgr2.startHeartbeat();
      `);

      vi.advanceTimersByTime(60000);

      expect(hbSandbox2.sentMessages).toHaveLength(0);

      vi.useRealTimers();
    });

    it('should send multiple heartbeats over time', () => {
      vi.useFakeTimers();

      const hbSandbox3 = createSandbox({
        setTimeout,
        setInterval,
        clearTimeout,
        clearInterval,
      });
      loadSourceWithExports(hbSandbox3.context, projectPath(SOURCE_FILE), ['WebSocketManager']);

      hbSandbox3.evalCode(`
        var _hbMgr3 = new WebSocketManager('ws://localhost/ws', 'token');
        _hbMgr3.ws = new WebSocket('ws://localhost/ws');
        _hbMgr3.ws.readyState = WebSocket.OPEN;
        _hbMgr3.startHeartbeat();
      `);

      vi.advanceTimersByTime(90000); // 3 intervals of 30s

      expect(hbSandbox3.sentMessages.length).toBe(3);

      vi.useRealTimers();
    });
  });

  // --- stopHeartbeat() ---

  describe('stopHeartbeat()', () => {
    it('should stop sending heartbeats after stop', () => {
      vi.useFakeTimers();

      const stopSandbox = createSandbox({
        setTimeout,
        setInterval,
        clearTimeout,
        clearInterval,
      });
      loadSourceWithExports(stopSandbox.context, projectPath(SOURCE_FILE), ['WebSocketManager']);

      stopSandbox.evalCode(`
        var _stopMgr = new WebSocketManager('ws://localhost/ws', 'token');
        _stopMgr.ws = new WebSocket('ws://localhost/ws');
        _stopMgr.ws.readyState = WebSocket.OPEN;
        _stopMgr.startHeartbeat();
      `);

      vi.advanceTimersByTime(30000);
      expect(stopSandbox.sentMessages).toHaveLength(1);

      stopSandbox.evalCode(`_stopMgr.stopHeartbeat()`);
      vi.advanceTimersByTime(60000);

      // Should still be 1 - no new heartbeats after stop
      expect(stopSandbox.sentMessages).toHaveLength(1);

      vi.useRealTimers();
    });
  });

  // --- disconnect() ---

  describe('disconnect()', () => {
    it('should set manualClose flag', () => {
      createManager('_dcMgr');
      sandbox.evalCode(`_dcMgr.disconnect()`);

      const manualClose = sandbox.evalCode(`_dcMgr.manualClose`);
      expect(manualClose).toBe(true);
    });
  });
});
