/* NemesisBot - Settings Page Component */

function settingsPage() {
  return {
    config: {},
    loading: true,
    error: '',

    init: function() {
      NemesisAPI.get('/api/config').then(function(data) {
        this.config = data;
        this.loading = false;
      }.bind(this)).catch(function(err) {
        console.error('[Settings] Failed to load:', err);
        this.error = '\u52A0\u8F7D\u914D\u7F6E\u5931\u8D25';
        this.loading = false;
      }.bind(this));
    },

    destroy: function() {
      // Nothing to clean up
    },

    formatValue: function(value) {
      if (value === null || value === undefined) return '--';
      if (typeof value === 'object') return JSON.stringify(value, null, 2);
      return String(value);
    },

    isSensitive: function(key) {
      var sensitive = ['key', 'token', 'secret', 'password', 'auth', 'credential'];
      var lower = key.toLowerCase();
      for (var i = 0; i < sensitive.length; i++) {
        if (lower.indexOf(sensitive[i]) !== -1) return true;
      }
      return false;

    },

    maskValue: function(key, value) {
      if (this.isSensitive(key) && typeof value === 'string' && value.length > 0) {
        return value.substring(0, 4) + '****';
      }
      return this.formatValue(value);
    }
  };
}
