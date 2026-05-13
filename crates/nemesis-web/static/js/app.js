/* NemesisBot - Alpine.js App Core */

// Register store via alpine:init event so Alpine.store() is available
// regardless of script load order (Alpine must be loaded with defer, after this file)
document.addEventListener('alpine:init', function() {
  Alpine.store('app', {
    connected: false,
    authenticated: false,
    page: 'chat',
    theme: 'dark',
    sidebarCollapsed: false,
    focusMode: false,
    version: '',
    token: ''
  });
});

// Main App Component
function app() {
  return {
    page: 'chat',
    sidebarCollapsed: false,
    loginToken: '',
    loginRemember: true,
    loginError: '',
    loginLoading: false,
    showMobileSidebar: false,

    init: function() {
      // Dashboard 子进程模式：检测 token 注入方式
      var tokenFromURL = '';

      // URL fragment token（备用方式）
      if (location.hash.indexOf('__dashboard_token=') !== -1) {
        var match = location.hash.match(/__dashboard_token=([^&#]+)/);
        if (match) {
          tokenFromURL = decodeURIComponent(match[1]);
          history.replaceState(null, '', location.pathname + location.search);
        }
      }

      // Hash routing（token 提取后 hash 已清理）
      this.page = location.hash.slice(1) || 'chat';
      window.addEventListener('hashchange', function() {
        Alpine.store('app').page = location.hash.slice(1) || 'chat';
      }.bind(this));

      // Theme
      var saved = localStorage.getItem('nemesisbot_theme');
      if (saved) {
        this.setTheme(saved);
      } else {
        // Check system preference
        if (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches) {
          this.setTheme('light');
        }
      }

      // Auto-login：按优先级尝试各种 token 来源
      if (tokenFromURL) {
        // Dashboard 子进程：URL fragment token
        this.autoLogin(tokenFromURL);
      } else if (window.__DASHBOARD_TOKEN__) {
        // Dashboard 子进程：反向代理注入的 token
        this.autoLogin(window.__DASHBOARD_TOKEN__);
      } else if (window.runtime && window.runtime.EventsOn) {
        // Dashboard 子进程：Wails 事件接收 token（备用）
        var self = this;
        window.runtime.EventsOn("dashboard-token", function(token) {
          if (token && !Alpine.store('app').authenticated) {
            self.autoLogin(token);
          }
        });
      } else {
        // 正常模式：尝试 localStorage auto-login
        var token = localStorage.getItem('nemesisbot_auth_token');
        if (token) {
          this.autoLogin(token);
        }
      }

      // Keyboard shortcuts
      document.addEventListener('keydown', function(e) {
        // Ctrl+B toggle sidebar
        if (e.ctrlKey && e.key === 'b') {
          e.preventDefault();
          this.toggleSidebar();
        }
      }.bind(this));

      // Watch for page changes from store
      this.$watch('$store.app.page', function(val) {
        this.page = val;
      }.bind(this));

      // Watch for auth changes
      this.$watch('$store.app.authenticated', function(val) {
        if (val) {
          // Connect SSE after auth
          NemesisAPI.connectEvents();
        }
      });
    },

    navigate: function(page) {
      this.page = page;
      location.hash = page;
      this.showMobileSidebar = false;
    },

    setTheme: function(theme) {
      this.$store.app.theme = theme;
      document.documentElement.setAttribute('data-theme', theme);
      localStorage.setItem('nemesisbot_theme', theme);
    },

    toggleTheme: function() {
      var current = this.$store.app.theme;
      this.setTheme(current === 'dark' ? 'light' : 'dark');
    },

    toggleSidebar: function() {
      this.sidebarCollapsed = !this.sidebarCollapsed;
      this.$store.app.sidebarCollapsed = this.sidebarCollapsed;
    },

    toggleMobileSidebar: function() {
      this.showMobileSidebar = !this.showMobileSidebar;
    },

    handleLogin: function() {
      var token = this.loginToken.trim();
      if (!token) {
        this.loginError = '\u8BF7\u8F93\u5165\u8BBF\u95EE\u5BC6\u94A5';
        return;
      }

      this.loginError = '';
      this.loginLoading = true;

      NemesisAPI.testConnection(token, function(success, err) {
        this.loginLoading = false;
        if (success) {
          if (this.loginRemember) {
            localStorage.setItem('nemesisbot_auth_token', token);
          }
          this.$store.app.token = token;
          this.$store.app.authenticated = true;
          NemesisAPI.token = token;
          NemesisAPI.connect(null, token);
        } else {
          this.loginError = err === 'Authentication failed'
            ? '\u8BBF\u95EE\u5BC6\u94A5\u65E0\u6548\uFF0C\u8BF7\u68C0\u67E5\u540E\u91CD\u8BD5'
            : '\u8FDE\u63A5\u5931\u8D25\uFF0C\u8BF7\u68C0\u67E5\u7F51\u7EDC\u6216\u670D\u52A1\u5668\u72B6\u6001';
        }
      }.bind(this));
    },

    autoLogin: function(token) {
      NemesisAPI.testConnection(token, function(success) {
        if (success) {
          this.$store.app.token = token;
          this.$store.app.authenticated = true;
          NemesisAPI.token = token;
          NemesisAPI.connect(null, token);
        } else {
          localStorage.removeItem('nemesisbot_auth_token');
        }
      }.bind(this));
    },

    handleLogout: function() {
      NemesisAPI.disconnect();
      NemesisAPI.disconnectEvents();
      localStorage.removeItem('nemesisbot_auth_token');
      this.$store.app.authenticated = false;
      this.$store.app.token = '';
      this.loginToken = '';
      this.loginError = '';
    },

    handleAuthInput: function(e) {
      if (e.key === 'Enter') {
        e.preventDefault();
        this.handleLogin();
      }
    }
  };
}
