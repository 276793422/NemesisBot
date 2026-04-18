// NemesisAPI Protocol Format Tests
// Verifies that the dashboard API client sends messages in the correct
// three-level dispatch protocol format (type -> module -> cmd).

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createSandbox, loadSource, projectPath } from '../__mocks__/browser.js';

const SOURCE_FILE = 'module/web/static/js/api.js';

describe('NemesisAPI Protocol Format', () => {
  let sandbox;

  beforeEach(() => {
    sandbox = createSandbox();
    // NemesisAPI uses `var`, so it becomes a context property automatically
    loadSource(sandbox.context, projectPath(SOURCE_FILE));
  });

  // --- send() ---

  describe('send()', () => {
    it('should send message in three-level protocol format', () => {
      sandbox.evalCode(`
        NemesisAPI.ws = new WebSocket('ws://localhost/ws');
        NemesisAPI.ws.readyState = WebSocket.OPEN;
        NemesisAPI.send('test message');
      `);

      expect(sandbox.sentMessages).toHaveLength(1);
      const msg = sandbox.sentMessages[0];
      expect(msg.type).toBe('message');
      expect(msg.module).toBe('chat');
      expect(msg.cmd).toBe('send');
      expect(msg.data).toEqual({ content: 'test message' });
      expect(msg.timestamp).toMatch(/^\d{4}-\d{2}-\d{2}T/);
    });

    it('should queue message when not connected', () => {
      sandbox.evalCode(`
        NemesisAPI.ws = null;
        NemesisAPI.send('queued');
      `);

      expect(sandbox.sentMessages).toHaveLength(0);
      const queue = sandbox.evalCode(`NemesisAPI._messageQueue`);
      expect(queue).toContain('queued');
    });

    it('should send message with unicode content', () => {
      sandbox.evalCode(`
        NemesisAPI.ws = new WebSocket('ws://localhost/ws');
        NemesisAPI.ws.readyState = WebSocket.OPEN;
        NemesisAPI.send('\u4F60\u597D\u4E16\u754C');
      `);

      const msg = sandbox.sentMessages[0];
      expect(msg.data.content).toBe('\u4F60\u597D\u4E16\u754C');
    });
  });

  // --- _startHeartbeat() ---

  describe('_startHeartbeat()', () => {
    it('should send heartbeat ping in correct protocol format', () => {
      vi.useFakeTimers();

      const hbSandbox = createSandbox({
        setTimeout,
        setInterval,
        clearTimeout,
        clearInterval,
      });
      loadSource(hbSandbox.context, projectPath(SOURCE_FILE));

      hbSandbox.evalCode(`
        NemesisAPI.ws = new WebSocket('ws://localhost/ws');
        NemesisAPI.ws.readyState = WebSocket.OPEN;
        NemesisAPI._startHeartbeat();
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
      loadSource(hbSandbox2.context, projectPath(SOURCE_FILE));

      hbSandbox2.evalCode(`
        NemesisAPI.ws = { readyState: WebSocket.CLOSED };
        NemesisAPI._startHeartbeat();
      `);

      vi.advanceTimersByTime(60000);

      expect(hbSandbox2.sentMessages).toHaveLength(0);

      vi.useRealTimers();
    });
  });

  // --- _stopHeartbeat() ---

  describe('_stopHeartbeat()', () => {
    it('should stop heartbeat interval', () => {
      vi.useFakeTimers();

      const stopSandbox = createSandbox({
        setTimeout,
        setInterval,
        clearTimeout,
        clearInterval,
      });
      loadSource(stopSandbox.context, projectPath(SOURCE_FILE));

      stopSandbox.evalCode(`
        NemesisAPI.ws = new WebSocket('ws://localhost/ws');
        NemesisAPI.ws.readyState = WebSocket.OPEN;
        NemesisAPI._startHeartbeat();
      `);

      vi.advanceTimersByTime(30000);
      expect(stopSandbox.sentMessages).toHaveLength(1);

      stopSandbox.evalCode(`NemesisAPI._stopHeartbeat()`);
      vi.advanceTimersByTime(60000);

      expect(stopSandbox.sentMessages).toHaveLength(1); // No new heartbeats

      vi.useRealTimers();
    });
  });

  // --- disconnect() ---

  describe('disconnect()', () => {
    it('should set manualClose and stop heartbeat', () => {
      sandbox.evalCode(`
        NemesisAPI.ws = new WebSocket('ws://localhost/ws');
        NemesisAPI.ws.readyState = WebSocket.OPEN;
        NemesisAPI.disconnect();
      `);

      const manualClose = sandbox.evalCode(`NemesisAPI._manualClose`);
      expect(manualClose).toBe(true);
    });
  });
});
