import { useState, useEffect } from 'react'
import { EventsOn } from '../wailsjs/runtime/runtime'
import {
  GetDesktopInfo, SimulateApproval, GetSimulatedRequests, GetApprovalHistory,
  GetApprovalStats, ClearApprovalHistory, SubmitApproval, GetLogs, GetLogModules,
  SendMessage, GetChatHistory, ClearChatHistory, GetSettings, UpdateSetting,
  GetSystemStatus, GetThemeConfig, SetTheme, GetKeyboardShortcuts
} from '../wailsjs/go/main/App'
import ApprovalDialog from './components/ApprovalDialog'
import './App.css'

function App() {
  const [desktopInfo, setDesktopInfo] = useState(null)
  const [showApproval, setShowApproval] = useState(false)
  const [approvalRequest, setApprovalRequest] = useState(null)
  const [currentPage, setCurrentPage] = useState('overview')
  const [simulatedRequests, setSimulatedRequests] = useState([])
  const [approvalHistory, setApprovalHistory] = useState([])
  const [approvalStats, setApprovalStats] = useState(null)

  // Chat state
  const [chatMessages, setChatMessages] = useState([])
  const [chatInput, setChatInput] = useState('')
  const [isSending, setIsSending] = useState(false)

  // Logs state
  const [logs, setLogs] = useState([])
  const [logLevel, setLogLevel] = useState('')
  const [logModule, setLogModule] = useState('')
  const [logModules, setLogModules] = useState([])

  // Settings state
  const [settings, setSettings] = useState([])
  const [systemStatus, setSystemStatus] = useState(null)
  const [theme, setTheme] = useState('dark')
  const [autoTheme, setAutoTheme] = useState(true)
  const [keyboardShortcuts, setKeyboardShortcuts] = useState([])

  useEffect(() => {
    loadDesktopInfo()
    loadSimulatedRequests()
    loadApprovalHistory()
    loadApprovalStats()
    loadThemeConfig()
    loadKeyboardShortcuts()

    // 监听审批请求事件
    const unlistenApproval = EventsOn("show-approval", (request) => {
      console.log('Received approval request:', request)
      setApprovalRequest(request)
      setShowApproval(true)
    })

    // 监听主题变更事件
    const unlistenTheme = EventsOn("theme-changed", (config) => {
      console.log('Theme changed:', config)
      setTheme(config.current_theme)
      setAutoTheme(config.auto_theme)
      // 应用主题到 body
      if (config.current_theme === 'light') {
        document.body.classList.add('light-theme')
        document.body.classList.remove('dark-theme')
      } else {
        document.body.classList.add('dark-theme')
        document.body.classList.remove('light-theme')
      }
    })

    // 监听快捷键执行事件
    const unlistenShortcut = EventsOn("execute-shortcut", (action) => {
      console.log('Executing shortcut:', action)
      handleKeyboardShortcut(action)
    })

    // 监听键盘事件
    const handleKeyDown = (e) => {
      // Ctrl+1-4: 导航到不同页面
      if (e.ctrlKey && e.key >= '1' && e.key <= '4') {
        e.preventDefault()
        const pages = ['chat', 'overview', 'logs', 'settings']
        setCurrentPage(pages[parseInt(e.key) - 1])
      }
      // Ctrl+K: 聚焦聊天输入框
      else if (e.ctrlKey && e.key === 'k') {
        e.preventDefault()
        if (currentPage === 'chat') {
          document.querySelector('.chat-input')?.focus()
        }
      }
      // Escape: 关闭对话框
      else if (e.key === 'Escape') {
        if (showApproval) {
          setShowApproval(false)
          setApprovalRequest(null)
        }
      }
    }

    window.addEventListener('keydown', handleKeyDown)

    return () => {
      if (unlistenApproval) unlistenApproval()
      if (unlistenTheme) unlistenTheme()
      if (unlistenShortcut) unlistenShortcut()
      window.removeEventListener('keydown', handleKeyDown)
    }
  }, [currentPage, showApproval])

  // 加载页面数据
  useEffect(() => {
    if (currentPage === 'chat') {
      loadChatHistory()
    } else if (currentPage === 'logs') {
      loadLogs()
      loadLogModules()
    } else if (currentPage === 'settings') {
      loadSettings()
      loadSystemStatus()
    }
  }, [currentPage, logLevel, logModule])

  const loadDesktopInfo = async () => {
    try {
      const info = await GetDesktopInfo()
      setDesktopInfo(info)
    } catch (error) {
      console.error('Failed to load desktop info:', error)
    }
  }

  const loadSimulatedRequests = async () => {
    try {
      const requests = await GetSimulatedRequests()
      setSimulatedRequests(requests)
    } catch (error) {
      console.error('Failed to load simulated requests:', error)
    }
  }

  const loadApprovalHistory = async () => {
    try {
      const history = await GetApprovalHistory(20)
      setApprovalHistory(history)
    } catch (error) {
      console.error('Failed to load approval history:', error)
    }
  }

  const loadApprovalStats = async () => {
    try {
      const stats = await GetApprovalStats()
      setApprovalStats(stats)
    } catch (error) {
      console.error('Failed to load approval stats:', error)
    }
  }

  const handleSimulateApproval = async (operation) => {
    try {
      const response = await SimulateApproval(operation)
      console.log('Approval response:', response)

      // Reload history and stats after approval
      await Promise.all([
        loadApprovalHistory(),
        loadApprovalStats()
      ])
    } catch (error) {
      console.error('Failed to simulate approval:', error)
    }
  }

  const handleClearHistory = async () => {
    try {
      await ClearApprovalHistory()
      await Promise.all([
        loadApprovalHistory(),
        loadApprovalStats()
      ])
    } catch (error) {
      console.error('Failed to clear history:', error)
    }
  }

  const handleApprovalDecision = async (approved) => {
    if (!approvalRequest) return

    try {
      const response = {
        request_id: approvalRequest.request_id,
        approved: approved,
        timed_out: false,
        duration_seconds: 0,
        response_time: Math.floor(Date.now() / 1000)
      }

      await SubmitApproval(response)
      console.log('Approval submitted:', response)

      // 关闭对话框
      setShowApproval(false)
      setApprovalRequest(null)

      // 重新加载历史和统计
      await Promise.all([
        loadApprovalHistory(),
        loadApprovalStats()
      ])
    } catch (error) {
      console.error('Failed to submit approval:', error)
    }
  }

  // Chat handlers
  const loadChatHistory = async () => {
    try {
      const history = await GetChatHistory(50)
      setChatMessages(history)
    } catch (error) {
      console.error('Failed to load chat history:', error)
    }
  }

  const handleSendMessage = async () => {
    if (!chatInput.trim() || isSending) return

    setIsSending(true)
    try {
      // 添加用户消息
      const userMsg = {
        id: `MSG-${Date.now()}`,
        role: 'user',
        content: chatInput,
        timestamp: Math.floor(Date.now() / 1000)
      }
      setChatMessages(prev => [...prev, userMsg])

      // 发送消息并获取响应
      const response = await SendMessage(chatInput)

      // 添加助手消息
      const assistantMsg = {
        id: `MSG-${Date.now() + 1}`,
        role: 'assistant',
        content: response,
        timestamp: Math.floor(Date.now() / 1000)
      }
      setChatMessages(prev => [...prev, assistantMsg])

      setChatInput('')
    } catch (error) {
      console.error('Failed to send message:', error)
    } finally {
      setIsSending(false)
    }
  }

  const handleClearChat = async () => {
    try {
      await ClearChatHistory()
      setChatMessages([])
    } catch (error) {
      console.error('Failed to clear chat history:', error)
    }
  }

  // Logs handlers
  const loadLogs = async () => {
    try {
      const logsData = await GetLogs(logLevel, logModule, 100)
      setLogs(logsData)
    } catch (error) {
      console.error('Failed to load logs:', error)
    }
  }

  const loadLogModules = async () => {
    try {
      const modules = await GetLogModules()
      setLogModules(modules)
    } catch (error) {
      console.error('Failed to load log modules:', error)
    }
  }

  // Settings handlers
  const loadSettings = async () => {
    try {
      const settingsData = await GetSettings()
      setSettings(settingsData)
    } catch (error) {
      console.error('Failed to load settings:', error)
    }
  }

  const loadSystemStatus = async () => {
    try {
      const status = await GetSystemStatus()
      setSystemStatus(status)
    } catch (error) {
      console.error('Failed to load system status:', error)
    }
  }

  const handleUpdateSetting = async (key, value) => {
    try {
      await UpdateSetting(key, value)
      await loadSettings()
    } catch (error) {
      console.error('Failed to update setting:', error)
    }
  }

  // Theme handlers
  const loadThemeConfig = async () => {
    try {
      const config = await GetThemeConfig()
      setTheme(config.current_theme)
      setAutoTheme(config.auto_theme)
    } catch (error) {
      console.error('Failed to load theme config:', error)
    }
  }

  const handleThemeChange = async (newTheme) => {
    try {
      await SetTheme(newTheme, false)
      setTheme(newTheme)
    } catch (error) {
      console.error('Failed to change theme:', error)
    }
  }

  const handleAutoThemeToggle = async (enabled) => {
    try {
      await SetTheme(theme, enabled)
      setAutoTheme(enabled)
    } catch (error) {
      console.error('Failed to toggle auto theme:', error)
    }
  }

  // Keyboard shortcuts handlers
  const loadKeyboardShortcuts = async () => {
    try {
      const shortcuts = await GetKeyboardShortcuts()
      setKeyboardShortcuts(shortcuts)
    } catch (error) {
      console.error('Failed to load keyboard shortcuts:', error)
    }
  }

  const handleKeyboardShortcut = (action) => {
    switch (action) {
      case 'navigate_chat':
        setCurrentPage('chat')
        break
      case 'navigate_overview':
        setCurrentPage('overview')
        break
      case 'navigate_logs':
        setCurrentPage('logs')
        break
      case 'navigate_settings':
        setCurrentPage('settings')
        break
      case 'focus_chat_input':
        if (currentPage === 'chat') {
          setTimeout(() => {
            document.querySelector('.chat-input')?.focus()
          }, 100)
        }
        break
      case 'clear_logs':
        if (currentPage === 'logs') {
          setLogs([])
        }
        break
      case 'clear_chat':
        handleClearChat()
        break
      case 'close_dialog':
        if (showApproval) {
          setShowApproval(false)
          setApprovalRequest(null)
        }
        break
      case 'quit':
        if (window.confirm('确定要退出应用吗？')) {
          window.close()
        }
        break
    }
  }

  const formatRiskLevel = (level) => {
    const colors = {
      'LOW': '🟢',
      'MEDIUM': '🟡',
      'HIGH': '🟠',
      'CRITICAL': '🔴'
    }
    return colors[level] || level
  }

  const formatTimestamp = (timestamp) => {
    return new Date(timestamp * 1000).toLocaleString('zh-CN')
  }

  return (
    <div className="app-container">
      {/* Header */}
      <div className="app-header">
        <div className="header-content">
          <h1>NemesisBot</h1>
          <p className="subtitle">v{desktopInfo?.version || '1.0.0'} | {desktopInfo?.bot_state || 'stopped'}</p>
        </div>
        <div className="header-actions">
          <button onClick={() => setCurrentPage('overview')} className="btn btn-secondary">
            审批中心
          </button>
        </div>
      </div>

      {/* Main Content */}
      <div className="app-main">
        <div className="sidebar">
          <div className="sidebar-nav">
            <div
              className={`nav-item ${currentPage === 'chat' ? 'active' : ''}`}
              onClick={() => setCurrentPage('chat')}
            >
              💬 Chat
            </div>
            <div
              className={`nav-item ${currentPage === 'overview' ? 'active' : ''}`}
              onClick={() => setCurrentPage('overview')}
            >
              📊 审批中心
            </div>
            <div
              className={`nav-item ${currentPage === 'logs' ? 'active' : ''}`}
              onClick={() => setCurrentPage('logs')}
            >
              📋 Logs
            </div>
            <div
              className={`nav-item ${currentPage === 'settings' ? 'active' : ''}`}
              onClick={() => setCurrentPage('settings')}
            >
              ⚙️ Settings
            </div>
          </div>
        </div>

        <div className="main-content">
          {currentPage === 'chat' && (
            <div className="page chat-page">
              <div className="chat-header">
                <h2>💬 Chat</h2>
                <button onClick={handleClearChat} className="btn btn-secondary btn-sm">
                  清空历史
                </button>
              </div>

              <div className="chat-container">
                <div className="chat-messages">
                  {chatMessages.length === 0 ? (
                    <div className="chat-empty">
                      <p>开始与 NemesisBot 对话...</p>
                    </div>
                  ) : (
                    chatMessages.map((msg) => (
                      <div key={msg.id} className={`chat-message chat-${msg.role}`}>
                        <div className="message-content">{msg.content}</div>
                        <div className="message-time">
                          {new Date(msg.timestamp * 1000).toLocaleTimeString('zh-CN')}
                        </div>
                      </div>
                    ))
                  )}
                  {isSending && (
                    <div className="chat-message chat-assistant">
                      <div className="message-content message-loading">
                        正在思考...
                      </div>
                    </div>
                  )}
                </div>

                <div className="chat-input-area">
                  <input
                    type="text"
                    className="chat-input"
                    placeholder="输入消息..."
                    value={chatInput}
                    onChange={(e) => setChatInput(e.target.value)}
                    onKeyPress={(e) => e.key === 'Enter' && handleSendMessage()}
                    disabled={isSending}
                  />
                  <button
                    onClick={handleSendMessage}
                    disabled={isSending || !chatInput.trim()}
                    className="btn btn-primary"
                  >
                    {isSending ? '发送中...' : '发送'}
                  </button>
                </div>
              </div>
            </div>
          )}

          {currentPage === 'overview' && (
            <div className="page overview-page">
              <div className="overview-header">
                <h2>📊 审批中心</h2>
                <button onClick={handleClearHistory} className="btn btn-secondary btn-sm">
                  清空历史
                </button>
              </div>

              {/* Statistics Cards */}
              {approvalStats && (
                <div className="stats-grid">
                  <div className="stat-card">
                    <div className="stat-label">总请求</div>
                    <div className="stat-value">{approvalStats.total_requests}</div>
                  </div>
                  <div className="stat-card stat-approved">
                    <div className="stat-label">已批准</div>
                    <div className="stat-value">{approvalStats.approved}</div>
                  </div>
                  <div className="stat-card stat-denied">
                    <div className="stat-label">已拒绝</div>
                    <div className="stat-value">{approvalStats.denied}</div>
                  </div>
                  <div className="stat-card stat-timeout">
                    <div className="stat-label">超时</div>
                    <div className="stat-value">{approvalStats.timeout}</div>
                  </div>
                  <div className="stat-card stat-avg">
                    <div className="stat-label">平均耗时</div>
                    <div className="stat-value">{approvalStats.avg_duration?.toFixed(2) || 0}s</div>
                  </div>
                </div>
              )}

              {/* Simulated Requests */}
              <div className="section">
                <h3>模拟审批请求</h3>
                <div className="request-buttons">
                  {simulatedRequests.map((req) => (
                    <button
                      key={req.id}
                      onClick={() => handleSimulateApproval(req.operation)}
                      className={`btn btn-request btn-${req.risk_level.toLowerCase()}`}
                    >
                      <span className="request-icon">{formatRiskLevel(req.risk_level)}</span>
                      <span className="request-name">{req.name}</span>
                    </button>
                  ))}
                </div>
              </div>

              {/* Approval History */}
              <div className="section">
                <h3>审批历史</h3>
                {approvalHistory.length === 0 ? (
                  <p className="text-muted">暂无审批记录</p>
                ) : (
                  <div className="history-list">
                    {approvalHistory.map((item) => (
                      <div key={item.request_id} className="history-item">
                        <div className="history-header">
                          <span className="history-id">{item.request_id}</span>
                          <span className={`history-status ${
                            item.approved ? 'status-approved' :
                            item.timed_out ? 'status-timeout' : 'status-denied'
                          }`}>
                            {item.approved ? '✓ 已批准' :
                             item.timed_out ? '⏱ 超时' : '✗ 已拒绝'}
                          </span>
                        </div>
                        <div className="history-body">
                          <div className="history-operation">{item.operation_name}</div>
                          <div className="history-target">{item.target}</div>
                          <div className="history-meta">
                            <span className="history-risk">{formatRiskLevel(item.risk_level)} {item.risk_level}</span>
                            <span className="history-time">{formatTimestamp(item.response_time)}</span>
                            <span className="history-duration">{item.duration_seconds?.toFixed(2)}s</span>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}

          {currentPage === 'logs' && (
            <div className="page logs-page">
              <div className="logs-header">
                <h2>📋 Logs</h2>
              </div>

              <div className="logs-filters">
                <select
                  value={logLevel}
                  onChange={(e) => setLogLevel(e.target.value)}
                  className="log-filter"
                >
                  <option value="">所有级别</option>
                  <option value="DEBUG">DEBUG</option>
                  <option value="INFO">INFO</option>
                  <option value="WARN">WARN</option>
                  <option value="ERROR">ERROR</option>
                </select>

                <select
                  value={logModule}
                  onChange={(e) => setLogModule(e.target.value)}
                  className="log-filter"
                >
                  <option value="">所有模块</option>
                  {logModules.map((mod) => (
                    <option key={mod} value={mod}>{mod}</option>
                  ))}
                </select>
              </div>

              <div className="logs-container">
                {logs.length === 0 ? (
                  <p className="text-muted">暂无日志</p>
                ) : (
                  logs.map((log, index) => (
                    <div key={index} className={`log-entry log-${log.level.toLowerCase()}`}>
                      <span className="log-timestamp">{log.timestamp}</span>
                      <span className="log-level">{log.level}</span>
                      <span className="log-module">[{log.module}]</span>
                      <span className="log-message">{log.message}</span>
                    </div>
                  ))
                )}
              </div>
            </div>
          )}

          {currentPage === 'settings' && (
            <div className="page settings-page">
              <h2>⚙️ Settings</h2>

              {/* System Status */}
              {systemStatus && (
                <div className="section">
                  <h3>系统状态</h3>
                  <div className="status-grid">
                    <div className="status-item">
                      <span className="status-label">运行时间</span>
                      <span className="status-value">{systemStatus.uptime}</span>
                    </div>
                    <div className="status-item">
                      <span className="status-label">内存使用</span>
                      <span className="status-value">{systemStatus.memory_usage_mb} MB</span>
                    </div>
                    <div className="status-item">
                      <span className="status-label">CPU 使用</span>
                      <span className="status-value">{systemStatus.cpu_usage_percent}%</span>
                    </div>
                    <div className="status-item">
                      <span className="status-label">线程数</span>
                      <span className="status-value">{systemStatus.thread_count}</span>
                    </div>
                    <div className="status-item">
                      <span className="status-label">版本</span>
                      <span className="status-value">{systemStatus.version}</span>
                    </div>
                    <div className="status-item">
                      <span className="status-label">Go 版本</span>
                      <span className="status-value">{systemStatus.go_version}</span>
                    </div>
                  </div>
                </div>
              )}

              {/* Settings */}
              <div className="section">
                <h3>配置项</h3>
                <div className="settings-list">
                  {settings.map((setting) => (
                    <div key={setting.key} className="setting-item">
                      <div className="setting-info">
                        <div className="setting-key">{setting.key}</div>
                        <div className="setting-description">{setting.description}</div>
                      </div>
                      <div className="setting-value">
                        {setting.type === 'boolean' ? (
                          <input
                            type="checkbox"
                            checked={setting.value === 'true'}
                            onChange={(e) => handleUpdateSetting(setting.key, e.target.checked.toString())}
                          />
                        ) : (
                          <input
                            type="text"
                            value={setting.value}
                            onChange={(e) => handleUpdateSetting(setting.key, e.target.value)}
                            className="setting-input"
                          />
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              </div>

              {/* Theme Settings */}
              <div className="section">
                <h3>主题设置</h3>
                <div className="theme-selector">
                  <div className="theme-options">
                    <button
                      onClick={() => handleThemeChange('dark')}
                      className={`theme-option ${theme === 'dark' ? 'active' : ''}`}
                    >
                      <span className="theme-icon">🌙</span>
                      <span className="theme-name">深色主题</span>
                    </button>
                    <button
                      onClick={() => handleThemeChange('light')}
                      className={`theme-option ${theme === 'light' ? 'active' : ''}`}
                    >
                      <span className="theme-icon">☀️</span>
                      <span className="theme-name">浅色主题</span>
                    </button>
                  </div>
                  <div className="auto-theme-toggle">
                    <label>
                      <input
                        type="checkbox"
                        checked={autoTheme}
                        onChange={(e) => handleAutoThemeToggle(e.target.checked)}
                      />
                      <span>自动切换主题</span>
                    </label>
                  </div>
                </div>
              </div>

              {/* Keyboard Shortcuts */}
              <div className="section">
                <h3>键盘快捷键</h3>
                <div className="shortcuts-list">
                  {keyboardShortcuts.map((shortcut, index) => (
                    <div key={index} className="shortcut-item">
                      <div className="shortcut-key">{shortcut.key}</div>
                      <div className="shortcut-action">{shortcut.description}</div>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Approval Dialog */}
      {showApproval && approvalRequest && (
        <ApprovalDialog
          request={approvalRequest}
          onApprove={() => handleApprovalDecision(true)}
          onDeny={() => handleApprovalDecision(false)}
          onClose={() => {
            setShowApproval(false)
            setApprovalRequest(null)
          }}
        />
      )}
    </div>
  )
}

export default App
