import { useState, useEffect, useRef } from 'react'
import './App.css'
import {
    GetDemoRequests,
    SubmitApproval,
    SimulateBackendRequest,
    GetSystemInfo
} from '../wailsjs/go/main/App'

function App() {
    const [currentRequest, setCurrentRequest] = useState(null)
    const [demoRequests, setDemoRequests] = useState([])
    const [countdown, setCountdown] = useState(30)
    const [isProcessing, setIsProcessing] = useState(false)
    const [approvedCount, setApprovedCount] = useState(0)
    const [deniedCount, setDeniedCount] = useState(0)
    const [showSimulation, setShowSimulation] = useState(false)
    const intervalRef = useRef(null)

    useEffect(() => {
        // 加载演示请求
        loadDemoRequests()
        return () => {
            if (intervalRef.current) {
                clearInterval(intervalRef.current)
            }
        }
    }, [])

    const loadDemoRequests = () => {
        GetDemoRequests().then(requests => {
            setDemoRequests(requests)
        }).catch(err => {
            console.error('Failed to load demo requests:', err)
        })
    }

    const startCountdown = (timeout) => {
        setCountdown(timeout)
        if (intervalRef.current) {
            clearInterval(intervalRef.current)
        }
        intervalRef.current = setInterval(() => {
            setCountdown(prev => {
                if (prev <= 1) {
                    clearInterval(intervalRef.current)
                    handleTimeout()
                    return 0
                }
                return prev - 1
            })
        }, 1000)
    }

    const selectRequest = (request) => {
        setCurrentRequest(request)
        startCountdown(request.TimeoutSeconds)
    }

    const simulateNewRequest = () => {
        const levels = ['LOW', 'MEDIUM', 'HIGH', 'CRITICAL']
        const randomLevel = levels[Math.floor(Math.random() * levels.length)]

        SimulateBackendRequest(randomLevel).then(request => {
            setCurrentRequest(request)
            setShowSimulation(false)
            startCountdown(request.TimeoutSeconds)
        }).catch(err => {
            console.error('Failed to simulate request:', err)
        })
    }

    const handleApprove = () => {
        if (!currentRequest || isProcessing) return

        setIsProcessing(true)
        clearInterval(intervalRef.current)

        const response = {
            request_id: currentRequest.RequestID,
            approved: true,
            timed_out: false,
            duration_seconds: (30 - countdown),
            response_time: Date.now() / 1000
        }

        SubmitApproval(response).then(() => {
            setApprovedCount(prev => prev + 1)
            setIsProcessing(false)
            setCurrentRequest(null)
        }).catch(err => {
            console.error('Failed to submit approval:', err)
            setIsProcessing(false)
        })
    }

    const handleDeny = () => {
        if (!currentRequest || isProcessing) return

        setIsProcessing(true)
        clearInterval(intervalRef.current)

        const response = {
            request_id: currentRequest.RequestID,
            approved: false,
            timed_out: false,
            duration_seconds: (30 - countdown),
            response_time: Date.now() / 1000
        }

        SubmitApproval(response).then(() => {
            setDeniedCount(prev => prev + 1)
            setIsProcessing(false)
            setCurrentRequest(null)
        }).catch(err => {
            console.error('Failed to submit approval:', err)
            setIsProcessing(false)
        })
    }

    const handleTimeout = () => {
        if (!currentRequest || isProcessing) return

        setIsProcessing(true)

        const response = {
            request_id: currentRequest.RequestID,
            approved: false,
            timed_out: true,
            duration_seconds: currentRequest.TimeoutSeconds,
            response_time: Date.now() / 1000
        }

        SubmitApproval(response).then(() => {
            setDeniedCount(prev => prev + 1)
            setIsProcessing(false)
            setCurrentRequest(null)
        }).catch(err => {
            console.error('Failed to submit approval:', err)
            setIsProcessing(false)
        })
    }

    const getRiskLevelClass = (level) => {
        const classes = {
            'LOW': 'risk-low',
            'MEDIUM': 'risk-medium',
            'HIGH': 'risk-high',
            'CRITICAL': 'risk-critical'
        }
        return classes[level] || 'risk-medium'
    }

    const getRiskLevelIcon = (level) => {
        const icons = {
            'LOW': '🟢',
            'MEDIUM': '🟡',
            'HIGH': '🟠',
            'CRITICAL': '🔴'
        }
        return icons[level] || '⚪'
    }

    return (
        <div className="app-container">
            <div className="approval-dialog">
                {/* Header */}
                <div className="dialog-header">
                    <div className="header-icon">
                        <span className="icon-warning">⚠️</span>
                        <span className="icon-lock">🔒</span>
                    </div>
                    <h1>安全审批请求</h1>
                    <p className="header-subtitle">AI Agent 需要您的授权才能执行此操作</p>
                </div>

                {/* Stats Bar */}
                <div className="stats-bar">
                    <div className="stat-item">
                        <span className="stat-label">已批准:</span>
                        <span className="stat-value stat-approved">{approvedCount}</span>
                    </div>
                    <div className="stat-item">
                        <span className="stat-label">已拒绝:</span>
                        <span className="stat-value stat-denied">{deniedCount}</span>
                    </div>
                    <button
                        className="btn-simulate"
                        onClick={() => setShowSimulation(true)}
                    >
                        模拟新请求
                    </button>
                </div>

                {/* Main Content */}
                {currentRequest ? (
                    <div className="dialog-content fade-in">
                        {/* Warning Message */}
                        <div className="warning-message">
                            <span className="warning-icon">⚡</span>
                            AI Agent 正在尝试执行危险操作
                        </div>

                        {/* Operation Name */}
                        <div className="operation-display">
                            <h2 className="operation-name">{currentRequest.OperationName}</h2>
                            <code className="operation-target">{currentRequest.Target}</code>
                        </div>

                        {/* Details */}
                        <div className="details-section">
                            <h3>操作详情</h3>

                            <div className="detail-row">
                                <span className="detail-label">操作类型:</span>
                                <span className="detail-value">{currentRequest.Operation}</span>
                            </div>

                            <div className="detail-row">
                                <span className="detail-label">操作目标:</span>
                                <span className="detail-value detail-target">{currentRequest.Target}</span>
                            </div>

                            <div className="detail-row">
                                <span className="detail-label">危险等级:</span>
                                <span className={`detail-value risk-badge ${getRiskLevelClass(currentRequest.RiskLevel)}`}>
                                    {getRiskLevelIcon(currentRequest.RiskLevel)} {currentRequest.RiskLevel}
                                </span>
                            </div>

                            <div className="detail-row">
                                <span className="detail-label">原因:</span>
                                <span className="detail-value">{currentRequest.Reason}</span>
                            </div>

                            {currentRequest.Context && Object.keys(currentRequest.Context).length > 0 && (
                                <div className="detail-row">
                                    <span className="detail-label">附加信息:</span>
                                    <div className="detail-context">
                                        {Object.entries(currentRequest.Context).map(([key, value]) => (
                                            <div key={key} className="context-item">
                                                <span className="context-key">{key}:</span>
                                                <span className="context-value">{value}</span>
                                            </div>
                                        ))}
                                    </div>
                                </div>
                            )}
                        </div>

                        {/* Countdown */}
                        <div className="countdown-section">
                            <div className="countdown-display">
                                <span className="countdown-icon">⏰</span>
                                <span className={`countdown-number ${countdown <= 10 ? 'countdown-urgent' : ''}`}>
                                    {countdown}
                                </span>
                                <span className="countdown-text">秒后自动拒绝</span>
                            </div>
                            <div className="countdown-bar">
                                <div
                                    className="countdown-progress"
                                    style={{
                                        width: `${(countdown / currentRequest.TimeoutSeconds) * 100}%`,
                                        backgroundColor: countdown <= 10 ? '#ef4444' : '#3b82f6'
                                    }}
                                />
                            </div>
                        </div>

                        {/* Action Buttons */}
                        <div className="action-buttons">
                            <button
                                className="btn btn-deny"
                                onClick={handleDeny}
                                disabled={isProcessing}
                            >
                                <span className="btn-icon">✗</span>
                                <span className="btn-text">拒绝操作</span>
                            </button>
                            <button
                                className="btn btn-approve"
                                onClick={handleApprove}
                                disabled={isProcessing}
                            >
                                <span className="btn-icon">✓</span>
                                <span className="btn-text">允许执行</span>
                            </button>
                        </div>

                        {isProcessing && (
                            <div className="processing-overlay">
                                <div className="spinner"></div>
                                <span>处理中...</span>
                            </div>
                        )}
                    </div>
                ) : (
                    <div className="dialog-content">
                        {showSimulation ? (
                            <div className="simulation-panel fade-in">
                                <h2>模拟新请求</h2>
                                <p className="simulation-desc">选择危险等级来模拟不同类型的审批请求</p>

                                <div className="risk-buttons">
                                    <button
                                        className="risk-btn risk-low"
                                        onClick={() => simulateNewRequest()}
                                    >
                                        🟢 LOW<br/>
                                        <span className="risk-btn-desc">低风险操作</span>
                                    </button>
                                    <button
                                        className="risk-btn risk-medium"
                                        onClick={() => simulateNewRequest()}
                                    >
                                        🟡 MEDIUM<br/>
                                        <span className="risk-btn-desc">中等风险操作</span>
                                    </button>
                                    <button
                                        className="risk-btn risk-high"
                                        onClick={() => simulateNewRequest()}
                                    >
                                        🟠 HIGH<br/>
                                        <span className="risk-btn-desc">高风险操作</span>
                                    </button>
                                    <button
                                        className="risk-btn risk-critical"
                                        onClick={() => simulateNewRequest()}
                                    >
                                        🔴 CRITICAL<br/>
                                        <span className="risk-btn-desc">严重风险操作</span>
                                    </button>
                                </div>

                                <button
                                    className="btn btn-back"
                                    onClick={() => setShowSimulation(false)}
                                >
                                    返回
                                </button>
                            </div>
                        ) : (
                            <div className="request-list fade-in">
                                <h2>待审批请求</h2>
                                <p className="list-desc">从下方选择一个请求进行审批，或创建新的模拟请求</p>

                                <div className="requests-container">
                                    {demoRequests.map((request, index) => (
                                        <div
                                            key={request.RequestID}
                                            className="request-card"
                                            onClick={() => selectRequest(request)}
                                            style={{ animationDelay: `${index * 0.1}s` }}
                                        >
                                            <div className="request-header">
                                                <span className={`request-risk ${getRiskLevelClass(request.RiskLevel)}`}>
                                                    {getRiskLevelIcon(request.RiskLevel)} {request.RiskLevel}
                                                </span>
                                                <span className="request-id">{request.RequestID}</span>
                                            </div>
                                            <h3 className="request-operation">{request.OperationName}</h3>
                                            <p className="request-target">{request.Target}</p>
                                        </div>
                                    ))}
                                </div>
                            </div>
                        )}
                    </div>
                )}

                {/* Footer */}
                <div className="dialog-footer">
                    <span className="footer-text">NemesisBot Security System v1.0</span>
                    <span className="footer-status">● 保护中</span>
                </div>
            </div>
        </div>
    )
}

export default App
