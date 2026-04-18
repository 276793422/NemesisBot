/* NemesisBot - Chat Page Component */

function chatPage() {
  return {
    messages: [],
    input: '',
    streaming: false,
    _wsCallback: null,
    _statusCallback: null,

    init: function() {
      // Subscribe to WebSocket messages
      this._wsCallback = function(data) {
        this.onMessage(data);
      }.bind(this);
      NemesisAPI.onMessage = this._wsCallback;

      this._statusCallback = function(status) {
        Alpine.store('app').connected = (status === 'connected');
      }.bind(this);
      NemesisAPI.onStatusChange = this._statusCallback;

      // Reconnect if needed
      var token = Alpine.store('app').token;
      if (token && (!NemesisAPI.ws || NemesisAPI.ws.readyState !== WebSocket.OPEN)) {
        NemesisAPI.connect(null, token);
      }
    },

    destroy: function() {
      // Don't disconnect WebSocket - other pages may still need it
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
      // Note: old flat format no longer supported after Phase 3 migration

      this.$nextTick(function() {
        this.scrollToBottom();
        this.renderCodeBlocks();
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
