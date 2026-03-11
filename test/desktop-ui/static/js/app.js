// NemesisBot Desktop UI - JavaScript

console.log('NemesisBot Desktop UI initializing...');

// Page navigation
document.addEventListener('DOMContentLoaded', function() {
    const navItems = document.querySelectorAll('.nav-item');
    const pages = document.querySelectorAll('.page');

    navItems.forEach(item => {
        item.addEventListener('click', function() {
            const pageName = this.getAttribute('data-page');
            if (!pageName) return;

            // Update active nav item
            navItems.forEach(nav => nav.classList.remove('active'));
            this.classList.add('active');

            // Show corresponding page
            pages.forEach(page => {
                if (page.id === 'page-' + pageName) {
                    page.style.display = '';
                } else {
                    page.style.display = 'none';
                }
            });

            console.log('Navigated to:', pageName);
        });
    });

    // Chat functionality
    const messageInput = document.getElementById('messageInput');
    const sendButton = document.getElementById('sendButton');
    const messagesContainer = document.getElementById('messages');

    function sendMessage() {
        const content = messageInput.value.trim();
        if (!content) return;

        // Add user message
        addMessage('user', content);

        // Clear input
        messageInput.value = '';
        messageInput.style.height = 'auto';

        // Call Go function if available (desktop mode)
        if (window.sendMessage) {
            try {
                const result = window.sendMessage(content);
                console.log('Go function result:', result);
            } catch (e) {
                console.log('Go sendMessage not available:', e);
            }
        }

        // Simulate response
        setTimeout(() => {
            addMessage('assistant', 'This is the NemesisBot Desktop UI prototype. The full version will connect to the AI agent system.');
        }, 500);
    }

    function addMessage(role, content) {
        const messageDiv = document.createElement('div');
        messageDiv.className = `message ${role}`;

        const contentDiv = document.createElement('div');
        contentDiv.className = 'message-content';
        contentDiv.textContent = content;

        const timeDiv = document.createElement('div');
        timeDiv.className = 'message-time';
        timeDiv.textContent = getCurrentTime();

        messageDiv.appendChild(contentDiv);
        messageDiv.appendChild(timeDiv);

        messagesContainer.appendChild(messageDiv);
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }

    function getCurrentTime() {
        const now = new Date();
        return now.toLocaleTimeString('en-US', {
            hour: '2-digit',
            minute: '2-digit',
            second: '2-digit'
        });
    }

    sendButton.addEventListener('click', sendMessage);

    messageInput.addEventListener('keydown', function(e) {
        if (e.ctrlKey && e.key === 'Enter') {
            e.preventDefault();
            sendMessage();
        }
    });

    // Auto-resize textarea
    messageInput.addEventListener('input', function() {
        this.style.height = 'auto';
        this.style.height = Math.min(this.scrollHeight, 150) + 'px';
    });

    // Initialize system info
    loadSystemInfo();

    // Update status
    updateConnectionStatus();

    console.log('NemesisBot Desktop UI initialized');
});

// Call Go function safely
function callGo(funcName, ...args) {
    if (window[funcName]) {
        try {
            return window[funcName](...args);
        } catch (e) {
            console.error('Error calling Go function:', funcName, e);
            return null;
        }
    }
    console.log('Go function not found:', funcName);
    return null;
}

// Load system information
function loadSystemInfo() {
    const config = callGo('getConfig');
    if (config) {
        console.log('System config:', config);
        updateConfigDisplay(config);
    }

    const version = callGo('getVersion');
    if (version) {
        console.log('Version:', version);
        const versionEl = document.querySelector('.sidebar-logo .version');
        if (versionEl) {
            versionEl.textContent = 'v' + version;
        }
    }
}

// Update configuration display
function updateConfigDisplay(config) {
    // Update status bar
    const statusText = document.getElementById('statusText');
    if (statusText && config.name) {
        statusText.textContent = `Connected to ${config.name}`;
    }

    // Update system info on overview page
    const systemInfoEl = document.querySelector('#page-overview .card-content');
    if (systemInfoEl) {
        systemInfoEl.innerHTML = `
            <div class="flex items-center gap-2 mb-4">
                <span class="status-dot connected"></span>
                <span>Agent Loop Running</span>
            </div>
            <div class="text-sm text-dim">OS: ${config.os || 'Unknown'}</div>
            <div class="text-sm text-dim">Arch: ${config.arch || 'Unknown'}</div>
            <div class="text-sm text-dim">Version: ${config.version || 'Unknown'}</div>
        `;
    }
}

// Update connection status
function updateConnectionStatus() {
    const statusDot = document.getElementById('statusDot');
    const statusText = document.getElementById('statusText');

    if (statusDot) {
        statusDot.className = 'status-dot connected';
    }

    if (statusText) {
        statusText.textContent = 'Connected to NemesisBot Desktop';
    }

    // Test health endpoint
    fetch('/health')
        .then(res => res.json())
        .then(data => {
            console.log('Health check:', data);
            if (statusText) {
                statusText.textContent = `Connected to ${data.name || 'NemesisBot'} (${data.version || '0.0.1'})`;
            }
        })
        .catch(err => {
            console.error('Health check failed:', err);
            if (statusDot) {
                statusDot.className = 'status-dot disconnected';
            }
            if (statusText) {
                statusText.textContent = 'Disconnected from NemesisBot';
            }
        });
}

// Theme switching
window.setTheme = function(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    console.log('Theme changed to:', theme);

    // Save preference
    try {
        localStorage.setItem('nemesisbot-theme', theme);
    } catch (e) {
        console.log('Could not save theme preference');
    }
};

// Load saved theme
function loadTheme() {
    try {
        const saved = localStorage.getItem('nemesisbot-theme');
        if (saved) {
            setTheme(saved);
        } else {
            // Default to dark theme
            setTheme('dark');
        }
    } catch (e) {
        setTheme('dark');
    }
}

// Load theme on startup
loadTheme();

// API helpers
async function apiCall(endpoint, method = 'GET', data = null) {
    const options = {
        method: method,
        headers: {
            'Content-Type': 'application/json'
        }
    };

    if (data) {
        options.body = JSON.stringify(data);
    }

    try {
        const response = await fetch(endpoint, options);
        return await response.json();
    } catch (e) {
        console.error('API call failed:', endpoint, e);
        return null;
    }
}

// Export functions for global access
window NemesisBotUI = {
    sendMessage: function(msg) {
        const input = document.getElementById('messageInput');
        if (input) {
            input.value = msg;
            // Trigger send
            input.dispatchEvent(new KeyboardEvent('keydown', {
                ctrlKey: true,
                key: 'Enter'
            }));
        }
    },

    navigateTo: function(page) {
        const navItem = document.querySelector(`[data-page="${page}"]`);
        if (navItem) {
            navItem.click();
        }
    },

    setTheme: setTheme,

    getConfig: function() {
        return callGo('getConfig');
    },

    getVersion: function() {
        return callGo('getVersion');
    }
};
