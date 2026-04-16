/* NemesisBot - Overview Page Component */

function overviewPage() {
  return {
    status: {},
    loading: true,
    _onStatus: null,

    init: function() {
      // Initial load
      NemesisAPI.get('/api/status').then(function(data) {
        this.status = data;
        this.loading = false;
      }.bind(this)).catch(function(err) {
        console.error('[Overview] Failed to load status:', err);
        this.loading = false;
      }.bind(this));

      // SSE real-time updates
      this._onStatus = function(data) {
        this.status = data;
        this.loading = false;
      }.bind(this);
      NemesisAPI.on('status', this._onStatus);
    },

    destroy: function() {
      if (this._onStatus) {
        NemesisAPI.off('status', this._onStatus);
      }
    },

    formatUptime: function(seconds) {
      if (!seconds) return '--';
      var d = Math.floor(seconds / 86400);
      var h = Math.floor((seconds % 86400) / 3600);
      var m = Math.floor((seconds % 3600) / 60);
      var s = seconds % 60;
      var parts = [];
      if (d > 0) parts.push(d + '\u5929');
      if (h > 0) parts.push(h + '\u5C0F\u65F6');
      if (m > 0) parts.push(m + '\u5206\u949F');
      if (s > 0 && d === 0) parts.push(s + '\u79D2');
      return parts.join(' ') || '0\u79D2';
    },

    getConnectionBadge: function(connected) {
      return connected ? '\u5DF2\u8FDE\u63A5' : '\u672A\u8FDE\u63A5';
    },

    getConnectionBadgeClass: function(connected) {
      return connected ? 'badge-success' : 'badge-error';
    }
  };
}
