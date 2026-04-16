/* NemesisBot - Logs Page Component */

function logsPage() {
  return {
    entries: [],
    source: 'general',
    level: '',
    filter: '',
    autoScroll: true,
    loading: false,
    _onLog: null,

    sources: [
      { id: 'general', label: '\u5E94\u7528\u65E5\u5FD7' },
      { id: 'llm', label: 'AI \u901A\u4FE1' },
      { id: 'security', label: '\u5B89\u5168\u5BA1\u8BA1' },
      { id: 'cluster', label: '\u96C6\u7FA4\u65E5\u5FD7' }
    ],

    levels: [
      { id: '', label: '\u5168\u90E8' },
      { id: 'DEBUG', label: 'DEBUG' },
      { id: 'INFO', label: 'INFO' },
      { id: 'WARN', label: 'WARN' },
      { id: 'ERROR', label: 'ERROR' }
    ],

    init: function() {
      this.loadInitial();

      this._onLog = function(entry) {
        if (entry.source && entry.source !== this.source) return;
        if (this.level && entry.level !== this.level) return;
        this.entries.push(entry);
        if (this.entries.length > 1000) {
          this.entries = this.entries.slice(-500);
        }
        if (this.autoScroll) {
          this.$nextTick(function() { this.scrollToBottom(); }.bind(this));
        }
      }.bind(this);
      NemesisAPI.on('log', this._onLog);
    },

    destroy: function() {
      if (this._onLog) {
        NemesisAPI.off('log', this._onLog);
      }
    },

    loadInitial: function() {
      this.loading = true;
      NemesisAPI.get('/api/logs?source=' + this.source + '&n=200').then(function(data) {
        this.entries = data.entries || [];
        this.loading = false;
        this.$nextTick(function() { this.scrollToBottom(); }.bind(this));
      }.bind(this)).catch(function(err) {
        console.error('[Logs] Failed to load:', err);
        this.loading = false;
      }.bind(this));
    },

    switchSource: function(src) {
      this.source = src;
      this.entries = [];
      this.loadInitial();
    },

    filteredEntries: function() {
      if (!this.filter) return this.entries;
      var f = this.filter.toLowerCase();
      return this.entries.filter(function(e) {
        return (e.message && e.message.toLowerCase().indexOf(f) !== -1) ||
               (e.component && e.component.toLowerCase().indexOf(f) !== -1);
      });
    },

    scrollToBottom: function() {
      var container = this.$refs.logsList;
      if (container) {
        container.scrollTop = container.scrollHeight;
      }
    },

    clearLogs: function() {
      this.entries = [];
    },

    formatTime: function(ts) {
      if (!ts) return '';
      var d = new Date(ts);
      return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
    },

    getLevelClass: function(level) {
      if (!level) return '';
      return 'log-level ' + level.toLowerCase();
    }
  };
}
