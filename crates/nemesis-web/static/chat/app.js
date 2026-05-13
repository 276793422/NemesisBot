// NemesisBot Web Chat - JavaScript Client

// Auth Manager - Handles token storage and authentication
class AuthManager {
    constructor() {
        this.storageKey = 'nemesisbot_auth_token';
    }

    // Save token to localStorage
    saveToken(token) {
        try {
            localStorage.setItem(this.storageKey, token);
            return true;
        } catch (e) {
            console.error('Failed to save token:', e);
            return false;
        }
    }

    // Get token from localStorage
    getToken() {
        try {
            return localStorage.getItem(this.storageKey);
        } catch (e) {
            console.error('Failed to get token:', e);
            return null;
        }
    }

    // Clear token (logout)
    clearToken() {
        try {
            localStorage.removeItem(this.storageKey);
            return true;
        } catch (e) {
            console.error('Failed to clear token:', e);
            return false;
        }
    }

    // Check if user is authenticated
    isAuthenticated() {
        return this.getToken() !== null;
    }
}

// WebSocket Manager - Uses three-level dispatch protocol
class WebSocketManager {
    constructor(url, authToken) {
        this.url = url;
        this.authToken = authToken;
        this.ws = null;
        this.reconnectDelay = 1000;
        this.maxReconnectDelay = 30000;
        this.messageQueue = [];
        this.onMessage = null;
        this.onStatusChange = null;
        this.onAuthError = null;
        this.manualClose = false;
    }

    connect() {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            return;
        }

        this.updateStatus('connecting');
        this.manualClose = false;

        try {
            // Build WebSocket URL with auth token
            let wsUrl = this.url;
            if (this.authToken) {
                const separator = wsUrl.includes('?') ? '&' : '?';
                wsUrl = wsUrl + separator + 'token=' + encodeURIComponent(this.authToken);
            }

            this.ws = new WebSocket(wsUrl);

            this.ws.onopen = () => {
                console.log('WebSocket connected');
                this.reconnectDelay = 1000;
                this.updateStatus('connected');

                // Send queued messages
                while (this.messageQueue.length > 0) {
                    const content = this.messageQueue.shift();
                    this.send(content);
                }
            };

            this.ws.onmessage = (event) => {
                try {
                    const data = JSON.parse(event.data);
                    console.log('Received:', data);

                    if (this.onMessage) {
                        this.onMessage(data);
                    }
                } catch (e) {
                    console.error('Failed to parse message:', e);
                }
            };

            this.ws.onclose = (event) => {
                console.log('WebSocket closed:', event.code, event.reason);
                this.ws = null;

                if (!this.manualClose) {
                    this.updateStatus('disconnected');

                    // Check if it's an auth error (close code 1008 or similar)
                    if (event.code === 1008 || event.code === 4001) {
                        // Authentication failed
                        if (this.onAuthError) {
                            this.onAuthError('Authentication failed. Please check your token.');
                        }
                    } else {
                        // Normal reconnect
                        this.reconnect();
                    }
                }
            };

            this.ws.onerror = (error) => {
                console.error('WebSocket error:', error);
                this.updateStatus('disconnected');
            };

        } catch (e) {
            console.error('Failed to create WebSocket:', e);
            this.updateStatus('disconnected');
            this.reconnect();
        }
    }

    // Update auth token and reconnect
    updateAuthToken(newToken) {
        this.authToken = newToken;
        if (this.ws) {
            this.manualClose = true;
            this.ws.close();
            this.ws = null;
        }
        this.connect();
    }

    // Send a chat message using new three-level protocol
    send(content) {
        const message = {
            type: 'message',
            module: 'chat',
            cmd: 'send',
            data: { content: content },
            timestamp: new Date().toISOString()
        };

        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(message));
        } else {
            console.log('Queueing message (not connected)');
            this.messageQueue.push(content);
            this.connect();
        }
    }

    // Request chat history using new protocol
    sendHistoryRequest(requestId, limit, beforeIndex) {
        const data = { request_id: requestId, limit: limit };
        if (beforeIndex !== null && beforeIndex !== undefined) {
            data.before_index = beforeIndex;
        }

        const message = {
            type: 'message',
            module: 'chat',
            cmd: 'history_request',
            data: data,
            timestamp: new Date().toISOString()
        };

        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(message));
        }
    }

    disconnect() {
        this.manualClose = true;
        if (this.ws) {
            this.ws.close();
            this.ws = null;
        }
        this.updateStatus('disconnected');
    }

    reconnect() {
        if (this.manualClose) {
            return;
        }

        console.log(`Reconnecting in ${this.reconnectDelay / 1000}s...`);
        setTimeout(() => {
            this.reconnectDelay = Math.min(this.reconnectDelay * 2, this.maxReconnectDelay);
            this.connect();
        }, this.reconnectDelay);
    }

    updateStatus(status) {
        if (this.onStatusChange) {
            this.onStatusChange(status);
        }
    }

    // Heartbeat uses new three-level protocol
    startHeartbeat() {
        this._heartbeatTimer = setInterval(() => {
            if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                this.ws.send(JSON.stringify({
                    type: 'system',
                    module: 'heartbeat',
                    cmd: 'ping',
                    data: {},
                    timestamp: new Date().toISOString()
                }));
            }
        }, 30000); // Every 30 seconds
    }

    stopHeartbeat() {
        if (this._heartbeatTimer) {
            clearInterval(this._heartbeatTimer);
            this._heartbeatTimer = null;
        }
    }
}

// Message Renderer
class MessageRenderer {
    constructor(container) {
        this.container = container;
    }

    appendMessage(role, content, timestamp, isError = false, isSystem = false) {
        const messageDiv = document.createElement('div');
        messageDiv.className = 'message';

        if (isError) {
            messageDiv.classList.add('error');
        } else if (isSystem) {
            messageDiv.classList.add('system');
        } else {
            messageDiv.classList.add(role);
        }

        // Format content
        if (role === 'assistant' && !isError && !isSystem) {
            messageDiv.innerHTML = this.renderMarkdown(content);
        } else {
            messageDiv.textContent = content;
        }

        // Add timestamp
        const timeDiv = document.createElement('div');
        timeDiv.className = 'message-time';
        timeDiv.textContent = this.formatTime(timestamp);
        messageDiv.appendChild(timeDiv);

        this.container.appendChild(messageDiv);
        this.scrollToBottom();

        // Apply syntax highlighting to code blocks (if library is loaded)
        if (role === 'assistant' && !isError && !isSystem && typeof hljs !== 'undefined') {
            messageDiv.querySelectorAll('pre code').forEach((block) => {
                hljs.highlightElement(block);
            });
        }
    }

    // Prepend history messages at the top of the container
    prependMessages(messages) {
        if (!messages || messages.length === 0) return;

        // Save current scroll position
        const oldScrollHeight = this.container.scrollHeight;

        // Create a document fragment for batch insertion
        const fragment = document.createDocumentFragment();

        for (const msg of messages) {
            const messageDiv = document.createElement('div');
            messageDiv.className = 'message';
            messageDiv.classList.add(msg.role);

            // Format content
            if (msg.role === 'assistant') {
                messageDiv.innerHTML = this.renderMarkdown(msg.content);
            } else {
                messageDiv.textContent = msg.content;
            }

            // Add timestamp
            if (msg.timestamp) {
                const timeDiv = document.createElement('div');
                timeDiv.className = 'message-time';
                timeDiv.textContent = this.formatTime(msg.timestamp);
                messageDiv.appendChild(timeDiv);
            }

            fragment.appendChild(messageDiv);
        }

        // Insert at the top
        this.container.insertBefore(fragment, this.container.firstChild);

        // Restore scroll position so the user sees the same content
        const newScrollHeight = this.container.scrollHeight;
        this.container.scrollTop = newScrollHeight - oldScrollHeight;

        // Apply syntax highlighting to newly added assistant messages
        if (typeof hljs !== 'undefined') {
            // Only highlight the newly prepended messages
            const children = this.container.children;
            for (let i = 0; i < messages.length && i < children.length; i++) {
                const child = children[i];
                if (child.classList.contains('assistant')) {
                    child.querySelectorAll('pre code').forEach((block) => {
                        hljs.highlightElement(block);
                    });
                }
            }
        }
    }

    renderMarkdown(text) {
        // Check if marked library is loaded
        if (typeof marked === 'undefined') {
            // Markdown library not loaded yet, return plain text
            return text.replace(/\n/g, '<br>');
        }

        // Configure marked options
        marked.setOptions({
            breaks: true,
            gfm: true,
            highlight: function(code, lang) {
                // Check if highlight.js is loaded
                if (typeof hljs === 'undefined') {
                    return code;
                }
                if (lang && hljs.getLanguage(lang)) {
                    try {
                        return hljs.highlight(code, { language: lang }).value;
                    } catch (e) {}
                }
                return hljs.highlightAuto(code).value;
            }
        });

        try {
            return marked.parse(text);
        } catch (e) {
            console.error('Markdown parsing error:', e);
            return text.replace(/\n/g, '<br>');
        }
    }

    formatTime(timestamp) {
        const date = new Date(timestamp);

        // 显示精确的北京时间，精确到毫秒
        const options = {
            timeZone: 'Asia/Shanghai',
            year: 'numeric',
            month: '2-digit',
            day: '2-digit',
            hour: '2-digit',
            minute: '2-digit',
            second: '2-digit',
            fractionalSecondDigits: 3,  // 精确到毫秒
            hour12: false
        };

        return date.toLocaleString('zh-CN', options);
    }

    scrollToBottom() {
        this.container.scrollTop = this.container.scrollHeight;
    }

    clear() {
        this.container.innerHTML = '';
    }
}

// UI Controller
class UIController {
    constructor() {
        this.wsManager = null;
        this.renderer = null;
        this.input = null;
        this.sendButton = null;
        this.statusIndicator = null;
        this.statusText = null;
        this.authManager = new AuthManager();

        // History loading state
        this._historyLoading = false;
        this._oldestIndex = null;
        this._hasMoreHistory = false;
        this._pendingHistoryRequests = new Map(); // requestId -> resolve callback
    }

    init() {
        // Check if user is authenticated
        if (this.authManager.isAuthenticated()) {
            this.showChatScreen();
            this.initChat(this.authManager.getToken());
        } else {
            this.showLoginScreen();
            this.initLogin();
        }
    }

    initLogin() {
        const loginButton = document.getElementById('login-button');
        const tokenInput = document.getElementById('auth-token-input');
        const rememberMe = document.getElementById('remember-me');
        const errorMessage = document.getElementById('login-error');

        // Focus on input
        tokenInput.focus();

        // Handle login button click
        loginButton.addEventListener('click', () => {
            this.handleLogin(tokenInput.value, rememberMe.checked, errorMessage);
        });

        // Handle Enter key
        tokenInput.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
                e.preventDefault();
                this.handleLogin(tokenInput.value, rememberMe.checked, errorMessage);
            }
        });
    }

    handleLogin(token, remember, errorElement) {
        const trimmedToken = token.trim();

        if (!trimmedToken) {
            errorElement.textContent = '请输入访问密钥';
            return;
        }

        // Clear error message
        errorElement.textContent = '';

        // Disable login button
        const loginButton = document.getElementById('login-button');
        loginButton.disabled = true;
        loginButton.textContent = '登录中...';

        // Try to connect with the token
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = protocol + '//' + window.location.host + '/ws';

        const testWsManager = new WebSocketManager(wsUrl, trimmedToken);

        let authSucceeded = false;
        let authFailed = false;

        // Set up callbacks
        testWsManager.onStatusChange = (status) => {
            if (status === 'connected') {
                authSucceeded = true;
                loginButton.disabled = false;
                loginButton.textContent = '登录';

                // Save token if remember me is checked
                if (remember) {
                    this.authManager.saveToken(trimmedToken);
                }

                // Close test connection and show chat screen
                testWsManager.manualClose = true;
                testWsManager.disconnect();

                this.showChatScreen();
                this.initChat(trimmedToken);
            }
        };

        testWsManager.onAuthError = (error) => {
            authFailed = true;
            loginButton.disabled = false;
            loginButton.textContent = '登录';
            errorElement.textContent = '访问密钥无效，请检查后重试';
            testWsManager.manualClose = true;
            testWsManager.disconnect();
        };

        // Timeout after 5 seconds
        setTimeout(() => {
            if (!authSucceeded && !authFailed) {
                loginButton.disabled = false;
                loginButton.textContent = '登录';
                errorElement.textContent = '连接超时，请检查网络或服务器状态';
                testWsManager.manualClose = true;
                testWsManager.disconnect();
            }
        }, 5000);

        // Try to connect
        testWsManager.connect();
    }

    handleLogout() {
        if (confirm('确定要退出登录吗？')) {
            // Disconnect WebSocket
            if (this.wsManager) {
                this.wsManager.stopHeartbeat();
                this.wsManager.manualClose = true;
                this.wsManager.disconnect();
            }

            // Clear token
            this.authManager.clearToken();

            // Show login screen
            this.showLoginScreen();
            this.initLogin();
        }
    }

    showLoginScreen() {
        document.getElementById('login-screen').style.display = '';
        document.getElementById('chat-screen').style.display = 'none';
    }

    showChatScreen() {
        document.getElementById('login-screen').style.display = 'none';
        document.getElementById('chat-screen').style.display = '';
    }

    initChat(authToken) {
        // Initialize renderer
        const messagesContainer = document.getElementById('messages-container');
        this.renderer = new MessageRenderer(messagesContainer);

        // Initialize input
        this.input = document.getElementById('message-input');
        this.sendButton = document.getElementById('send-button');

        // Initialize status indicator
        this.statusIndicator = document.querySelector('.status-dot');
        this.statusText = document.querySelector('.status-text');

        // Initialize logout button
        const logoutButton = document.getElementById('logout-button');
        logoutButton.addEventListener('click', () => this.handleLogout());

        // Initialize WebSocket manager
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = protocol + '//' + window.location.host + '/ws';
        this.wsManager = new WebSocketManager(wsUrl, authToken);

        // Set up callbacks
        this.wsManager.onMessage = (data) => this.handleMessage(data);
        this.wsManager.onStatusChange = (status) => {
            this.updateStatus(status);
            // Load history when connected
            if (status === 'connected') {
                this.loadHistory();
            }
        };
        this.wsManager.onAuthError = (error) => {
            // Auth error during chat session - token might have expired
            alert('认证失败：' + error + '\n请重新登录');
            this.handleLogout();
        };

        // Bind events
        this.sendButton.addEventListener('click', () => this.sendMessage());
        this.input.addEventListener('keydown', (e) => {
            if (e.ctrlKey && e.key === 'Enter') {
                e.preventDefault();
                this.sendMessage();
            }
        });

        // Auto-resize textarea
        this.input.addEventListener('input', () => {
            this.input.style.height = 'auto';
            this.input.style.height = Math.min(this.input.scrollHeight, 150) + 'px';
        });

        // Setup scroll listener for loading more history
        this.setupScrollListener(messagesContainer);

        // Connect to WebSocket
        this.wsManager.connect();
        this.wsManager.startHeartbeat();

        // Focus input
        this.input.focus();
    }

    // Setup scroll listener to load more history when scrolling to top
    setupScrollListener(container) {
        container.addEventListener('scroll', () => {
            if (container.scrollTop < 50 && this._hasMoreHistory && !this._historyLoading) {
                this.loadHistory();
            }
        });
    }

    // Load history messages
    loadHistory() {
        if (this._historyLoading) return;

        this._historyLoading = true;
        const requestId = 'hist_' + Date.now();

        const limit = 20;
        const beforeIndex = this._oldestIndex;

        this.wsManager.sendHistoryRequest(requestId, limit, beforeIndex);
    }

    // Handle incoming messages (new three-level protocol only)
    handleMessage(data) {
        if (data.type === 'message' && data.module === 'chat') {
            switch (data.cmd) {
                case 'history':
                    this.handleHistoryResponse(data.data);
                    break;
                case 'receive':
                    this.renderer.appendMessage(
                        data.data.role || 'assistant',
                        data.data.content,
                        data.timestamp
                    );
                    this.enableInput();
                    break;
            }
        } else if (data.type === 'system' && data.module === 'heartbeat' && data.cmd === 'pong') {
            console.log('Pong received');
        } else if (data.type === 'system' && data.module === 'error' && data.cmd === 'notify') {
            this.renderer.appendMessage('', data.data.content || data.data, data.timestamp, true);
            this.enableInput();
        }
    }

    // Handle history response
    handleHistoryResponse(data) {
        this._historyLoading = false;

        if (!data) return;

        const messages = data.messages || [];

        if (messages.length > 0) {
            this.renderer.prependMessages(messages);
        }

        this._hasMoreHistory = data.has_more || false;
        this._oldestIndex = data.oldest_index;

        // If this is the initial load (no messages existed before), scroll to bottom
        if (data.total_count > 0 && this._oldestIndex === 0) {
            // All history loaded, no more to fetch
            this._hasMoreHistory = false;
        }
    }

    sendMessage() {
        const content = this.input.value.trim();
        if (!content) {
            return;
        }

        // Disable input while sending
        this.disableInput();

        // Display user message
        this.renderer.appendMessage('user', content, new Date().toISOString());

        // Clear input
        this.input.value = '';
        this.input.style.height = 'auto';

        // Send to server (uses new protocol)
        this.wsManager.send(content);
    }

    disableInput() {
        this.input.disabled = true;
        this.sendButton.disabled = true;
        this.sendButton.textContent = '发送中...';
    }

    enableInput() {
        this.input.disabled = false;
        this.sendButton.disabled = false;
        this.sendButton.textContent = '发送';
        this.input.focus();
    }

    updateStatus(status) {
        // Remove all status classes
        this.statusIndicator.classList.remove('connecting', 'connected', 'disconnected');

        // Add new status class
        this.statusIndicator.classList.add(status);

        // Update status text
        const statusTexts = {
            'connecting': '连接中...',
            'connected': '已连接',
            'disconnected': '已断开'
        };
        this.statusText.textContent = statusTexts[status] || status;

        // Enable/disable input based on status
        if (status === 'connected') {
            this.enableInput();
        } else if (status === 'disconnected') {
            this.disableInput();
        }
    }
}

// Initialize on page load
document.addEventListener('DOMContentLoaded', () => {
    const controller = new UIController();
    controller.init();
});
