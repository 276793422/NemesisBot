/* NemesisBot - Chat Page Component */

function chatPage() {
  return {
    messages: [],
    input: '',
    streaming: false,
    _wsCallback: null,
    _statusCallback: null,

    // History state
    _hasMoreHistory: true,
    _historyLoading: false,
    _oldestIndex: null,
    _historyLoaded: false,
    _scrollHandler: null,

    init: function() {
      // Subscribe to WebSocket messages
      this._wsCallback = function(data) {
        this.onMessage(data);
      }.bind(this);
      NemesisAPI.onMessage = this._wsCallback;

      this._statusCallback = function(status) {
        Alpine.store('app').connected = (status === 'connected');

        // Load history on first connection
        if (status === 'connected' && !this._historyLoaded) {
          this.loadHistory();
        }
      }.bind(this);
      NemesisAPI.onStatusChange = this._statusCallback;

      // Setup scroll listener for loading more history
      this.setupScrollListener();

      // Reconnect if needed
      var token = Alpine.store('app').token;
      if (token && (!NemesisAPI.ws || NemesisAPI.ws.readyState !== WebSocket.OPEN)) {
        NemesisAPI.connect(null, token);
      } else if (NemesisAPI.ws && NemesisAPI.ws.readyState === WebSocket.OPEN && !this._historyLoaded) {
        this.loadHistory();
      }
    },

    destroy: function() {
      // Remove scroll listener
      if (this._scrollHandler && this.$refs.chatMessages) {
        this.$refs.chatMessages.removeEventListener('scroll', this._scrollHandler);
        this._scrollHandler = null;
      }
    },

    send: function() {
      var content = this.input.trim();
      if (!content || this.streaming) return;

      // Add user message to UI
      this.messages.push({
        role: 'user',
        content: content,
        timestamp: new Date().toISOString()
      });

      this.input = '';
      this.streaming = true;

      // Reset textarea height
      var ta = this.$refs.chatInput;
      if (ta) ta.style.height = 'auto';

      NemesisAPI.send(content);
      this.scrollToBottom();
    },

    onMessage: function(data) {
      // Handle new three-level protocol format
      if (data.module !== undefined) {
        if (data.type === 'message' && data.module === 'chat') {
          if (data.cmd === 'receive') {
            this.messages.push({
              role: data.data.role || 'assistant',
              content: data.data.content,
              timestamp: data.timestamp
            });
            this.streaming = false;
          } else if (data.cmd === 'history') {
            this.handleHistoryResponse(data.data);
          }
        } else if (data.type === 'system' && data.module === 'error' && data.cmd === 'notify') {
          this.messages.push({
            role: 'error',
            content: data.data.content || data.data,
            timestamp: data.timestamp
          });
          this.streaming = false;
        }
        // Ignore heartbeat.pong and other system messages
      }

      this.$nextTick(function() {
        this.scrollToBottom();
        this.renderCodeBlocks();
      }.bind(this));
    },

    // Load history from server
    loadHistory: function() {
      if (this._historyLoading) return;

      this._historyLoading = true;
      var requestId = 'hist_' + Date.now();
      var limit = 20;
      var beforeIndex = this._oldestIndex;

      NemesisAPI.sendHistoryRequest(requestId, limit, beforeIndex);
    },

    // Handle history response from server
    handleHistoryResponse: function(data) {
      this._historyLoading = false;
      if (!data) return;

      var historyMessages = data.messages || [];

      if (historyMessages.length > 0) {
        // Remember scroll position before prepending
        var container = this.$refs.chatMessages;
        var oldScrollHeight = container ? container.scrollHeight : 0;

        // Prepend history messages to the top
        var newMessages = historyMessages.map(function(m) {
          return {
            role: m.role,
            content: m.content,
            timestamp: m.timestamp || new Date().toISOString()
          };
        });
        this.messages = newMessages.concat(this.messages);

        // Restore scroll position after messages are prepended
        this.$nextTick(function() {
          if (container) {
            var newScrollHeight = container.scrollHeight;
            container.scrollTop = newScrollHeight - oldScrollHeight;
          }
        });
      }

      this._hasMoreHistory = data.has_more || false;
      this._oldestIndex = data.oldest_index;
      this._historyLoaded = true;

      // Scroll to bottom on initial load (first page of history)
      if (this._oldestIndex === 0 || !data.has_more) {
        this._hasMoreHistory = false;
        this.$nextTick(function() {
          this.scrollToBottom();
        }.bind(this));
      }
    },

    // Setup scroll listener to load more history when scrolled to top
    setupScrollListener: function() {
      var self = this;
      this._scrollHandler = function() {
        var container = self.$refs.chatMessages;
        if (!container) return;

        if (container.scrollTop <= 50 && self._hasMoreHistory && !self._historyLoading && self._historyLoaded) {
          self.loadHistory();
        }
      };

      this.$nextTick(function() {
        var container = this.$refs.chatMessages;
        if (container) {
          container.addEventListener('scroll', this._scrollHandler);
        }
      }.bind(this));
    },

    renderMarkdown: function(text) {
      if (typeof marked === 'undefined') {
        return text.replace(/\n/g, '<br>');
      }
      marked.setOptions({
        breaks: true,
        gfm: true,
        highlight: function(code, lang) {
          if (typeof hljs === 'undefined') return code;
          if (lang && hljs.getLanguage(lang)) {
            try { return hljs.highlight(code, { language: lang }).value; } catch (e) {}
          }
          return hljs.highlightAuto(code).value;
        }
      });
      try {
        return marked.parse(text);
      } catch (e) {
        return text.replace(/\n/g, '<br>');
      }
    },

    renderCodeBlocks: function() {
      // Apply highlight.js to any new code blocks
      this.$nextTick(function() {
        var blocks = this.$el.querySelectorAll('pre code:not(.hljs)');
        blocks.forEach(function(block) {
          if (typeof hljs !== 'undefined') {
            hljs.highlightElement(block);
          }
        });
      }.bind(this));
    },

    formatTime: function(timestamp) {
      var date = new Date(timestamp);
      return date.toLocaleTimeString('zh-CN', {
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        hour12: false
      });
    },

    scrollToBottom: function() {
      var container = this.$refs.chatMessages;
      if (container) {
        container.scrollTop = container.scrollHeight;
      }
    },

    handleKeydown: function(e) {
      if (e.ctrlKey && e.key === 'Enter') {
        e.preventDefault();
        this.send();
      }
    },

    handleInput: function(e) {
      var el = e.target;
      el.style.height = 'auto';
      el.style.height = Math.min(el.scrollHeight, 150) + 'px';
    },

    getAvatar: function(role) {
      if (role === 'user') return 'U';
      return 'NB';
    }
  };
}
