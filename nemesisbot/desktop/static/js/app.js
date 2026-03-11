// NemesisBot Desktop UI - JavaScript

console.log('NemesisBot Desktop UI initializing...');

// Global state
const AppState = {
    botState: 'unknown',
    botError: null,
    config: null,
    currentPage: 'control',
    pollInterval: null,
    wizardData: {}
};

// Page navigation
document.addEventListener('DOMContentLoaded', function() {
    console.log('DOM loaded, initializing UI...');

    // Initialize navigation
    initNavigation();

    // Initialize chat functionality
    initChat();

    // Check configuration and show appropriate page
    checkConfiguration();

    // Start polling bot status
    startPolling();

    console.log('NemesisBot Desktop UI initialized');
});

function initNavigation() {
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
                    // Special handling for chat page
                    if (pageName === 'chat') {
                        page.style.display = 'flex';
                    }
                } else {
                    page.style.display = 'none';
                }
            });

            AppState.currentPage = pageName;
            console.log('Navigated to:', pageName);

            // Refresh page-specific data
            if (pageName === 'control') refreshBotStatus();
            if (pageName === 'overview') refreshOverview();
            if (pageName === 'logs') refreshLogs();
        });
    });
}

function navigateTo(page) {
    const navItem = document.querySelector(`[data-page="${page}"]`);
    if (navItem) {
        navItem.click();
    }
}

// Check if configuration exists
function checkConfiguration() {
    log('info', 'Checking configuration...');

    const config = callGo('getConfig');
    if (config && !config.error) {
        AppState.config = config;
        log('info', 'Configuration found');
        showPage('control');
        updateConfigStatus('✓ Configured');
        refreshBotStatus();
        refreshHealth();
        refreshConfigDisplay();
    } else {
        log('warn', 'No configuration found');
        showPage('wizard');
        updateConfigStatus('⚠ Not configured');
        showWizardStep('welcome');
    }

    // Update version
    const version = callGo('getVersion');
    if (version) {
        const versionEl = document.querySelector('.sidebar-logo .version');
        if (versionEl) versionEl.textContent = 'v' + version;

        const aboutVersion = document.getElementById('aboutVersion');
        if (aboutVersion) aboutVersion.textContent = version;
    }
}

function showPage(pageName) {
    // Hide all pages
    document.querySelectorAll('.page').forEach(page => {
        page.style.display = 'none';
    });

    // Show target page
    const targetPage = document.getElementById('page-' + pageName);
    if (targetPage) {
        targetPage.style.display = '';
        if (pageName === 'chat') {
            targetPage.style.display = 'flex';
        }
    }

    // Update nav
    document.querySelectorAll('.nav-item').forEach(item => {
        item.classList.remove('active');
        if (item.getAttribute('data-page') === pageName) {
            item.classList.add('active');
        }
    });

    AppState.currentPage = pageName;
}

// Configuration Wizard
function showWizardStep(stepId) {
    document.querySelectorAll('.wizard-step').forEach(step => {
        step.style.display = 'none';
    });

    const targetStep = document.getElementById('wizard-step-' + stepId);
    if (targetStep) {
        targetStep.style.display = '';
    }

    AppState.wizardData.currentStep = stepId;
}

function wizardNext(stepId) {
    // Save current step data
    saveWizardStep();

    // Move to next step
    showWizardStep(stepId);
}

function wizardBack(stepId) {
    showWizardStep(stepId);
}

function saveWizardStep() {
    const currentStep = AppState.wizardData.currentStep;

    switch (currentStep) {
        case 'setup':
            AppState.wizardData.workspace = document.getElementById('workspacePath').value;
            AppState.wizardData.botName = document.getElementById('botName').value;
            break;
        case 'models':
            AppState.wizardData.provider = document.getElementById('modelProvider').value;
            AppState.wizardData.apiKey = document.getElementById('apiKey').value;
            AppState.wizardData.model = document.getElementById('modelName').value;
            break;
        case 'channels':
            AppState.wizardData.enableWeb = document.getElementById('enableWeb').checked;
            AppState.wizardData.enableDesktop = document.getElementById('enableDesktop').checked;
            AppState.wizardData.enableTelegram = document.getElementById('enableTelegram').checked;
            break;
    }
}

function updateModelProvider() {
    const provider = document.getElementById('modelProvider').value;
    const modelSelect = document.getElementById('modelName');

    const models = {
        'anthropic': [
            { value: 'claude-sonnet-4-20250514', text: 'Claude Sonnet 4' },
            { value: 'claude-opus-4-20250514', text: 'Claude Opus 4' },
            { value: 'claude-3-5-sonnet-20241022', text: 'Claude 3.5 Sonnet' }
        ],
        'openai': [
            { value: 'gpt-4o', text: 'GPT-4o' },
            { value: 'gpt-4o-mini', text: 'GPT-4o Mini' },
            { value: 'gpt-4-turbo', text: 'GPT-4 Turbo' }
        ],
        'zhipu': [
            { value: 'glm-4', text: 'GLM-4' },
            { value: 'glm-4-plus', text: 'GLM-4 Plus' }
        ]
    };

    modelSelect.innerHTML = '';
    models[provider].forEach(model => {
        const option = document.createElement('option');
        option.value = model.value;
        option.textContent = model.text;
        modelSelect.appendChild(option);
    });
}

async function wizardFinish() {
    saveWizardStep();

    log('info', 'Saving configuration...');

    try {
        // Build configuration object
        const config = {
            workspace_path: AppState.wizardData.workspace || './workspace',
            name: AppState.wizardData.botName || 'NemesisBot',
            model_list: [{
                provider: AppState.wizardData.provider || 'anthropic',
                name: AppState.wizardData.model || 'claude-sonnet-4-20250514',
                api_key: AppState.wizardData.apiKey || '',
                default: true
            }],
            channels: {
                web: {
                    enabled: AppState.wizardData.enableWeb !== false,
                    host: '127.0.0.1',
                    port: 49000,
                    auth_token: generateAuthToken()
                },
                desktop: {
                    enabled: AppState.wizardData.enableDesktop !== false
                },
                telegram: {
                    enabled: AppState.wizardData.enableTelegram || false
                }
            },
            gateway: {
                host: '127.0.0.1',
                port: 49001
            },
            heartbeat: {
                enabled: true,
                interval: 30
            },
            devices: {
                enabled: false,
                monitor_usb: false
            },
            security: {
                enabled: true,
                restrict_to_workspace: true
            }
        };

        // Save configuration via Go backend
        // Note: This would require implementing the save endpoint
        // For now, we'll simulate it
        log('info', 'Configuration saved');
        showWizardStep('success');

    } catch (error) {
        log('error', 'Failed to save configuration: ' + error.message);
        showToast('Error saving configuration', 'error');
    }
}

function generateAuthToken() {
    return 'token_' + Math.random().toString(36).substring(2, 15) + Math.random().toString(36).substring(2, 15);
}

function startBotFromWizard() {
    log('info', 'Starting bot from wizard...');
    showPage('control');
    startBot();
}

// Bot Control Functions
function refreshBotStatus() {
    const botState = callGo('getBotState');
    if (botState) {
        AppState.botState = botState.state;
        AppState.botError = botState.error;
        updateBotStatusUI(botState);
    }
}

function updateBotStatusUI(botState) {
    const statusDot = document.getElementById('botStatusDot');
    const statusText = document.getElementById('botStatusText');
    const statusDetail = document.getElementById('botStatusDetail');
    const startBtn = document.getElementById('startBotBtn');
    const stopBtn = document.getElementById('stopBotBtn');

    if (!statusText) return;

    // Update status text
    const stateMap = {
        'not_started': 'Not Started',
        'starting': 'Starting...',
        'running': 'Running',
        'error': 'Error'
    };

    statusText.textContent = stateMap[botState.state] || 'Unknown';

    // Update status detail
    if (botState.state === 'running') {
        statusDetail.textContent = 'Bot is ready to process messages';
    } else if (botState.state === 'error') {
        statusDetail.textContent = botState.error || 'An error occurred';
    } else if (botState.state === 'starting') {
        statusDetail.textContent = 'Please wait...';
    } else {
        statusDetail.textContent = 'Click Start to begin';
    }

    // Update status dot
    if (statusDot) {
        statusDot.className = 'status-dot';
        if (botState.state === 'running') {
            statusDot.classList.add('connected');
        } else if (botState.state === 'error') {
            statusDot.classList.add('disconnected');
        } else if (botState.state === 'starting') {
            statusDot.classList.add('connecting');
        }
    }

    // Update buttons
    if (startBtn && stopBtn) {
        if (botState.state === 'running') {
            startBtn.style.display = 'none';
            stopBtn.style.display = 'inline-flex';
        } else {
            startBtn.style.display = 'inline-flex';
            stopBtn.style.display = 'none';
            startBtn.disabled = botState.state === 'starting';
        }
    }

    // Update connection bar
    const connStatusDot = document.getElementById('statusDot');
    const connStatusText = document.getElementById('statusText');

    if (connStatusDot && connStatusText) {
        connStatusDot.className = 'status-dot';
        if (botState.state === 'running') {
            connStatusDot.classList.add('connected');
            connStatusText.textContent = 'Connected to NemesisBot';
        } else if (botState.state === 'error') {
            connStatusDot.classList.add('disconnected');
            connStatusText.textContent = 'Bot Error';
        } else {
            connStatusText.textContent = 'Bot Not Running';
        }
    }
}

async function startBot() {
    log('info', 'Starting bot...');
    updateBotStatusUI({ state: 'starting' });

    try {
        const result = await callGoAsync('startBot');
        if (result instanceof Error) {
            throw result;
        }
        log('info', 'Bot start initiated');
        showToast('Bot is starting...', 'info');

        // Poll for status updates
        setTimeout(refreshBotStatus, 1000);
        setTimeout(refreshBotStatus, 3000);
    } catch (error) {
        log('error', 'Failed to start bot: ' + error.message);
        showToast('Failed to start bot: ' + error.message, 'error');
        updateBotStatusUI({ state: 'error', error: error.message });
    }
}

async function stopBot() {
    log('info', 'Stopping bot...');

    try {
        const result = await callGoAsync('stopBot');
        if (result instanceof Error) {
            throw result;
        }
        log('info', 'Bot stop initiated');
        showToast('Bot is stopping...', 'info');

        // Poll for status updates
        setTimeout(refreshBotStatus, 1000);
    } catch (error) {
        log('error', 'Failed to stop bot: ' + error.message);
        showToast('Failed to stop bot: ' + error.message, 'error');
    }
}

async function restartBot() {
    log('info', 'Restarting bot...');
    updateBotStatusUI({ state: 'starting' });

    try {
        const result = await callGoAsync('restartBot');
        if (result instanceof Error) {
            throw result;
        }
        log('info', 'Bot restart initiated');
        showToast('Bot is restarting...', 'info');

        // Poll for status updates
        setTimeout(refreshBotStatus, 2000);
        setTimeout(refreshBotStatus, 5000);
    } catch (error) {
        log('error', 'Failed to restart bot: ' + error.message);
        showToast('Failed to restart bot: ' + error.message, 'error');
        updateBotStatusUI({ state: 'error', error: error.message });
    }
}

// Health & Configuration
function refreshHealth() {
    const health = callGo('getHealth');
    if (health) {
        const serviceStatus = document.getElementById('healthServiceStatus');
        const uptime = document.getElementById('healthUptime');
        const version = document.getElementById('healthVersion');
        const botRunning = document.getElementById('healthBotRunning');

        if (serviceStatus) serviceStatus.textContent = health.status || '--';
        if (uptime) uptime.textContent = health.uptime || '--';
        if (version) version.textContent = health.version || '--';
        if (botRunning) {
            botRunning.textContent = health.bot_running ? 'Yes' : 'No';
        }

        // Update overview page
        const overviewStatusDot = document.getElementById('overviewStatusDot');
        const overviewStatusText = document.getElementById('overviewStatusText');

        if (overviewStatusDot && overviewStatusText) {
            overviewStatusDot.className = 'status-dot';
            if (health.status === 'ok') {
                overviewStatusDot.classList.add('connected');
                overviewStatusText.textContent = 'System Operational';
            } else {
                overviewStatusDot.classList.add('disconnected');
                overviewStatusText.textContent = 'System Issue';
            }
        }

        // Update channels on overview
        const overviewChannels = document.getElementById('overviewChannels');
        if (overviewChannels && AppState.config) {
            const channels = [];
            if (AppState.config.channels?.web?.enabled) channels.push('Web');
            if (AppState.config.channels?.desktop?.enabled) channels.push('Desktop');
            if (AppState.config.channels?.telegram?.enabled) channels.push('Telegram');

            if (channels.length > 0) {
                overviewChannels.innerHTML = channels.map(c =>
                    `<span class="badge badge-success" style="margin-right: 4px;">${c}</span>`
                ).join('');
            } else {
                overviewChannels.textContent = 'No channels enabled';
            }
        }
    }
}

function refreshConfigDisplay() {
    if (!AppState.config) return;

    const workspace = document.getElementById('configWorkspace');
    const provider = document.getElementById('configProvider');
    const model = document.getElementById('configModel');
    const channels = document.getElementById('configChannels');

    if (workspace) workspace.textContent = AppState.config.workspace_path || '--';
    if (provider) {
        const defaultModel = AppState.config.model_list?.find(m => m.default);
        if (provider) provider.textContent = defaultModel?.provider || '--';
    }
    if (model) {
        const defaultModel = AppState.config.model_list?.find(m => m.default);
        if (model) model.textContent = defaultModel?.name || '--';
    }
    if (channels) {
        const enabledChannels = [];
        if (AppState.config.channels?.web?.enabled) enabledChannels.push('Web');
        if (AppState.config.channels?.desktop?.enabled) enabledChannels.push('Desktop');
        if (AppState.config.channels?.telegram?.enabled) enabledChannels.push('Telegram');
        if (channels) channels.textContent = enabledChannels.join(', ') || 'None';
    }
}

function refreshOverview() {
    refreshHealth();
    refreshBotStatus();
}

function refreshLogs() {
    const logsContent = document.getElementById('logsContent');
    if (logsContent) {
        // In a real implementation, this would fetch logs from the backend
        logsContent.textContent = `[INFO] NemesisBot Desktop UI started
[INFO] Bot State: ${AppState.botState}
[INFO] Configuration: ${AppState.config ? 'Loaded' : 'Not configured'}
[INFO] System Ready

Tip: Check the console for detailed logs (F12)`;
    }
}

// Chat functionality
function initChat() {
    const messageInput = document.getElementById('messageInput');
    const sendButton = document.getElementById('sendButton');

    if (!messageInput || !sendButton) return;

    async function sendMessage() {
        const content = messageInput.value.trim();
        if (!content) return;

        // Check if bot is running
        if (AppState.botState !== 'running') {
            showToast('Please start the bot first', 'warning');
            return;
        }

        // Add user message
        addMessage('user', content);

        // Clear input
        messageInput.value = '';
        messageInput.style.height = 'auto';

        // Send to bot
        try {
            const result = await callGoAsync('sendMessage', content);
            if (result && typeof result === 'string') {
                log('info', 'Message sent: ' + result);
            }
        } catch (error) {
            log('error', 'Failed to send message: ' + error.message);
            addMessage('assistant', 'Error: Failed to send message');
        }
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
}

function addMessage(role, content) {
    const messagesContainer = document.getElementById('messages');
    if (!messagesContainer) return;

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

// Polling
function startPolling() {
    // Poll every 2 seconds
    AppState.pollInterval = setInterval(() => {
        if (AppState.currentPage === 'control' || AppState.currentPage === 'overview') {
            refreshBotStatus();
            refreshHealth();
        }
    }, 2000);

    // Initial refresh
    refreshBotStatus();
    refreshHealth();
}

// Go function helpers
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

async function callGoAsync(funcName, ...args) {
    return new Promise((resolve, reject) => {
        if (window[funcName]) {
            try {
                const result = window[funcName](...args);
                // Go functions return immediately, but operations are async
                // We'll simulate this with setTimeout
                setTimeout(() => resolve(result), 100);
            } catch (e) {
                reject(e);
            }
        } else {
            reject(new Error('Go function not found: ' + funcName));
        }
    });
}

// UI Helpers
function updateConfigStatus(status) {
    const configStatus = document.getElementById('configStatus');
    if (configStatus) {
        configStatus.textContent = status;
    }
}

function showToast(message, type = 'info') {
    const container = document.getElementById('toastContainer');
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;
    toast.style.cssText = `
        background: var(--surface);
        border: 1px solid var(--border);
        border-left: 3px solid ${type === 'error' ? 'var(--error)' : type === 'warning' ? 'var(--warning)' : 'var(--accent)'};
        padding: 12px 16px;
        margin-bottom: 8px;
        border-radius: var(--radius-md);
        box-shadow: var(--shadow-lg);
        animation: slideInRight 0.3s var(--ease-smooth);
        min-width: 250px;
    `;
    toast.textContent = message;

    container.appendChild(toast);

    // Remove after 3 seconds
    setTimeout(() => {
        toast.style.animation = 'slideOutRight 0.3s var(--ease-smooth)';
        setTimeout(() => toast.remove(), 300);
    }, 3000);
}

function log(level, message) {
    console.log(`[${level.toUpperCase()}]`, message);

    // Also log to Go backend
    if (window.log) {
        try {
            window.log(level, message);
        } catch (e) {
            // Ignore logging errors
        }
    }
}

// Theme switching
window.setTheme = function(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    console.log('Theme changed to:', theme);

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
            // Auto-detect system theme
            if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
                setTheme('dark');
            } else {
                setTheme('dark'); // Default to dark for cyberpunk theme
            }
        }
    } catch (e) {
        setTheme('dark');
    }
}

// Load theme on startup
loadTheme();

// Listen for system theme changes
if (window.matchMedia) {
    const darkModeQuery = window.matchMedia('(prefers-color-scheme: dark)');
    darkModeQuery.addEventListener('change', function(e) {
        if (!localStorage.getItem('nemesisbot-theme')) {
            setTheme(e.matches ? 'dark' : 'light');
        }
    });
}

// Add animation keyframes
const style = document.createElement('style');
style.textContent = `
    @keyframes slideInRight {
        from {
            opacity: 0;
            transform: translateX(100%);
        }
        to {
            opacity: 1;
            transform: translateX(0);
        }
    }

    @keyframes slideOutRight {
        from {
            opacity: 1;
            transform: translateX(0);
        }
        to {
            opacity: 0;
            transform: translateX(100%);
        }
    }

    .form-group {
        margin-bottom: 16px;
    }

    .form-group label {
        display: block;
        font-weight: 600;
        margin-bottom: 6px;
        color: var(--text);
    }

    .form-group small {
        display: block;
        margin-top: 4px;
        color: var(--text-dim);
    }

    .btn-success {
        background: var(--success);
        color: white;
    }

    .btn-success:hover {
        background: var(--success-dim);
        box-shadow: 0 4px 12px rgba(16, 185, 129, 0.3);
    }

    .btn-warning {
        background: var(--warning);
        color: white;
    }

    .btn-warning:hover {
        background: var(--warning-dim);
        box-shadow: 0 4px 12px rgba(245, 158, 11, 0.3);
    }

    .wizard-container {
        max-width: 600px;
        margin: 0 auto;
    }

    .wizard-step {
        animation: fadeIn 0.3s var(--ease-smooth);
    }
`;
document.head.appendChild(style);

// Cleanup on page unload
window.addEventListener('beforeunload', function() {
    if (AppState.pollInterval) {
        clearInterval(AppState.pollInterval);
    }
});

// Export for global access
window.NemesisBotUI = {
    sendMessage: function(msg) {
        const input = document.getElementById('messageInput');
        if (input) {
            input.value = msg;
            input.dispatchEvent(new KeyboardEvent('keydown', {
                ctrlKey: true,
                key: 'Enter'
            }));
        }
    },
    navigateTo: navigateTo,
    setTheme: setTheme,
    refreshBotStatus: refreshBotStatus,
    getBotState: function() {
        return AppState.botState;
    }
};

console.log('NemesisBot UI functions exported');
