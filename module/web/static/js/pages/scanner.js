/* NemesisBot - Scanner Page Component */

function scannerPage() {
  return {
    engines: [],
    loading: true,
    _onProgress: null,

    init: function() {
      NemesisAPI.get('/api/scanner/status').then(function(data) {
        this.engines = data.engines || [];
        this.loading = false;
      }.bind(this)).catch(function(err) {
        console.error('[Scanner] Failed to load:', err);
        this.loading = false;
      }.bind(this));

      this._onProgress = function(data) {
        // Update progress for matching engine
        for (var i = 0; i < this.engines.length; i++) {
          if (this.engines[i].name === data.engine) {
            this.engines[i].progress = data;
            break;
          }
        }
      }.bind(this);
      NemesisAPI.on('scanner-progress', this._onProgress);
    },

    destroy: function() {
      if (this._onProgress) {
        NemesisAPI.off('scanner-progress', this._onProgress);
      }
    },

    getStateBadge: function(state) {
      var map = {
        'pending': '\u5F85\u5B89\u88C5',
        'installed': '\u5DF2\u5B89\u88C5',
        'failed': '\u5B89\u88C5\u5931\u8D25',
        'ready': '\u5C31\u7EEA',
        'stale': '\u9700\u66F4\u65B0',
        'running': '\u8FD0\u884C\u4E2D'
      };
      return map[state] || state;
    },

    getStateBadgeClass: function(state) {
      var map = {
        'pending': 'badge-neutral',
        'installed': 'badge-info',
        'failed': 'badge-error',
        'ready': 'badge-success',
        'stale': 'badge-warning',
        'running': 'badge-success'
      };
      return map[state] || 'badge-neutral';
    }
  };
}
