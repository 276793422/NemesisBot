/* NemesisBot - API Client + WebSocket + SSE + Toast */

var NemesisAPI = {
  ws: null,
  token: null,
  _reconnectDelay: 1000,
  _maxReconnectDelay: 30000,
  _messageQueue: [],
  _manualClose: false,
  _heartbeatInterval: null,

  // Callbacks
  onMessage: null,
  onStatusChange: null,

  // SSE
  _eventSource: null,
  _eventHandlers: {},

  // Connect WebSocket
  connect: function(host, token) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      return;
    }

    this.token = token;
    this._manualClose = false;
    this._notifyStatus('connecting');

    var wsUrl = host || this._buildWSUrl();
    if (token) {
      var sep = wsUrl.indexOf('?') !== -1 ? '&' : '?';
      wsUrl = wsUrl + sep + 'token=' + encodeURIComponent(token);
    }

    try {
      this.ws = new WebSocket(wsUrl);

      this.ws.onopen = function() {
        console.log('[NemesisAPI] WebSocket connected');
        NemesisAPI._reconnectDelay = 1000;
        NemesisAPI._notifyStatus('connected');
        NemesisAPI._flushQueue();
        NemesisAPI._startHeartbeat();
      };

      this.ws.onmessage = function(event) {
        try {
          var data = JSON.parse(event.data);
          if (NemesisAPI.onMessage) {
            NemesisAPI.onMessage(data);
          }
        } catch (e) {
          console.error('[NemesisAPI] Parse error:', e);
        }
      };

      this.ws.onclose = function(event) {
        console.log('[NemesisAPI] WebSocket closed:', event.code);
        NemesisAPI.ws = null;
        NemesisAPI._stopHeartbeat();

        if (!NemesisAPI._manualClose) {
          NemesisAPI._notifyStatus('disconnected');
          if (event.code === 1008 || event.code === 4001) {
            NemesisAPI._notifyStatus('auth_error');
          } else {
            NemesisAPI._reconnect();
          }
        }
      };

      this.ws.onerror = function() {
        NemesisAPI._notifyStatus('disconnected');
      };

    } catch (e) {
      console.error('[NemesisAPI] Connect error:', e);
      this._notifyStatus('disconnected');
      this._reconnect();
    }
  },

  // Send message (new three-level protocol)
  send: function(content) {
    var message = {
      type: 'message',
      module: 'chat',
      cmd: 'send',
      data: { content: content },
      timestamp: new Date().toISOString()
    };

    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message));
    } else {
      this._messageQueue.push(content);
      this.connect();
    }
  },

  // Send history request
  sendHistoryRequest: function(requestId, limit, beforeIndex) {
    var data = { request_id: requestId, limit: limit };
    if (beforeIndex !== null && beforeIndex !== undefined) {
      data.before_index = beforeIndex;
    }
    var message = {
      type: 'message',
      module: 'chat',
      cmd: 'history_request',
      data: data,
      timestamp: new Date().toISOString()
    };
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message));
    }
  },

  // Disconnect
  disconnect: function() {
    this._manualClose = true;
    this._stopHeartbeat();
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this._notifyStatus('disconnected');
  },

  // Test connection with token
  testConnection: function(token, callback) {
    var wsUrl = this._buildWSUrl();
    var sep = wsUrl.indexOf('?') !== -1 ? '&' : '?';
    wsUrl = wsUrl + sep + 'token=' + encodeURIComponent(token);

    var testWs = new WebSocket(wsUrl);
    var done = false;

    testWs.onopen = function() {
      if (!done) {
        done = true;
        testWs.close();
        callback(true);
      }
    };

    testWs.onerror = function() {
      if (!done) {
        done = true;
        callback(false, 'Connection failed');
      }
    };

    testWs.onclose = function(event) {
      if (!done) {
        done = true;
        if (event.code === 1008 || event.code === 4001) {
          callback(false, 'Authentication failed');
        } else {
          callback(false, 'Connection closed');
        }
      }
    };

    setTimeout(function() {
      if (!done) {
        done = true;
        testWs.close();
        callback(false, 'Connection timeout');
      }
    }, 5000);
  },

  // HTTP GET helper
  get: function(path) {
    return fetch(path).then(function(res) {
      if (!res.ok) throw new Error('HTTP ' + res.status);
      return res.json();
    });
  },

  // SSE - Connect to event stream
  connectEvents: function() {
    if (this._eventSource) return;

    try {
      this._eventSource = new EventSource('/api/events/stream');

      this._eventSource.onopen = function() {
        console.log('[NemesisAPI] SSE connected');
      };

      this._eventSource.onerror = function() {
        console.log('[NemesisAPI] SSE error, will auto-reconnect');
      };

      // Register event type listeners
      var eventTypes = ['log', 'status', 'security-alert', 'scanner-progress', 'cluster-event', 'heartbeat'];
      eventTypes.forEach(function(type) {
        NemesisAPI._eventSource.addEventListener(type, function(e) {
          try {
            var data = JSON.parse(e.data);
            NemesisAPI._dispatch(type, data);
          } catch (err) {
            console.error('[NemesisAPI] SSE parse error:', err);
          }
        });
      });
    } catch (e) {
      console.error('[NemesisAPI] SSE connect error:', e);
    }
  },

  disconnectEvents: function() {
    if (this._eventSource) {
      this._eventSource.close();
      this._eventSource = null;
    }
  },

  // SSE - Subscribe to event type
  on: function(eventType, handler) {
    if (!this._eventHandlers[eventType]) {
      this._eventHandlers[eventType] = [];
    }
    this._eventHandlers[eventType].push(handler);
  },

  // SSE - Unsubscribe from event type
  off: function(eventType, handler) {
    if (!this._eventHandlers[eventType]) return;
    if (!handler) {
      delete this._eventHandlers[eventType];
      return;
    }
    this._eventHandlers[eventType] = this._eventHandlers[eventType].filter(function(h) {
      return h !== handler;
    });
  },

  // SSE - Dispatch event to subscribers
  _dispatch: function(eventType, data) {
    var handlers = this._eventHandlers[eventType] || [];
    handlers.forEach(function(h) { h(data); });
  },

  // Internal
  _buildWSUrl: function() {
    if (window.__DASHBOARD_BACKEND__) {
      return 'ws://' + window.__DASHBOARD_BACKEND__ + '/ws';
    }
    var protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    return protocol + '//' + window.location.host + '/ws';
  },

  _flushQueue: function() {
    while (this._messageQueue.length > 0) {
      var msg = this._messageQueue.shift();
      this.send(msg.content);
    }
  },

  _reconnect: function() {
    if (this._manualClose) return;
    console.log('[NemesisAPI] Reconnecting in ' + this._reconnectDelay + 'ms...');
    setTimeout(function() {
      NemesisAPI._reconnectDelay = Math.min(NemesisAPI._reconnectDelay * 2, NemesisAPI._maxReconnectDelay);
      NemesisAPI.connect(null, NemesisAPI.token);
    }, this._reconnectDelay);
  },

  _notifyStatus: function(status) {
    if (this.onStatusChange) {
      this.onStatusChange(status);
    }
  },

  _startHeartbeat: function() {
    this._stopHeartbeat();
    this._heartbeatInterval = setInterval(function() {
      if (NemesisAPI.ws && NemesisAPI.ws.readyState === WebSocket.OPEN) {
        NemesisAPI.ws.send(JSON.stringify({
          type: 'system',
          module: 'heartbeat',
          cmd: 'ping',
          data: {},
          timestamp: new Date().toISOString()
        }));
      }
    }, 30000);
  },

  _stopHeartbeat: function() {
    if (this._heartbeatInterval) {
      clearInterval(this._heartbeatInterval);
      this._heartbeatInterval = null;
    }
  }
};

/* ===== Toast Notification System ===== */
var NemesisToast = {
  _container: null,

  _getContainer: function() {
    if (!this._container) {
      this._container = document.getElementById('toast-container');
    }
    return this._container;
  },

  show: function(message, type, duration) {
    type = type || 'info';
    duration = duration || 4000;

    var container = this._getContainer();
    if (!container) return;

    var icons = {
      info: '\u2139\uFE0F',
      success: '\u2705',
      warn: '\u26A0\uFE0F',
      error: '\u274C'
    };

    var titles = {
      info: '\u4FE1\u606F',
      success: '\u6210\u529F',
      warn: '\u8B66\u544A',
      error: '\u9519\u8BEF'
    };

    var toast = document.createElement('div');
    toast.className = 'toast ' + type;
    toast.innerHTML =
      '<div class="toast-icon">' + (icons[type] || '') + '</div>' +
      '<div class="toast-body">' +
        '<div class="toast-title">' + (titles[type] || type) + '</div>' +
        '<div class="toast-message">' + this._escapeHtml(message) + '</div>' +
      '</div>' +
      '<button class="toast-close" onclick="NemesisToast._remove(this.parentElement)">&times;</button>';

    container.appendChild(toast);

    if (duration > 0) {
      setTimeout(function() {
        NemesisToast._remove(toast);
      }, duration);
    }
  },

  info: function(msg) { this.show(msg, 'info'); },
  success: function(msg) { this.show(msg, 'success'); },
  warn: function(msg) { this.show(msg, 'warn'); },
  error: function(msg) { this.show(msg, 'error', 6000); },

  _remove: function(el) {
    if (!el || !el.parentElement) return;
    el.classList.add('removing');
    setTimeout(function() {
      if (el.parentElement) el.parentElement.removeChild(el);
    }, 200);
  },

  _escapeHtml: function(str) {
    var div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }
};
