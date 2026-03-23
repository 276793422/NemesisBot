import { useState, useEffect } from 'react'
import './ApprovalDialog.css'

function ApprovalDialog({ request, onApprove, onDeny, onClose }) {
  const [countdown, setCountdown] = useState(request?.timeout_seconds || 30)
  const [isProcessing, setIsProcessing] = useState(false)

  useEffect(() => {
    if (!request) return

    const timer = setInterval(() => {
      setCountdown(prev => {
        if (prev <= 1) {
          handleTimeout()
          return 0
        }
        return prev - 1
      })
    }, 1000)

    return () => clearInterval(timer)
  }, [request])

  const handleApprove = async () => {
    if (isProcessing) return
    setIsProcessing(true)

    if (onApprove) {
      await onApprove()
    }
  }

  const handleDeny = async () => {
    if (isProcessing) return
    setIsProcessing(true)

    if (onDeny) {
      await onDeny()
    }
  }

  const handleTimeout = async () => {
    if (onDeny) {
      await onDeny()
    }
  }

  if (!request) {
    return null
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

  return (
    <div className="approval-dialog-overlay">
      <div className="approval-dialog fade-in">
        {/* Header */}
        <div className="dialog-header">
          <div className="header-icon">
            <span className="icon-warning">⚠️</span>
            <span className="icon-lock">🔒</span>
          </div>
          <h1>安全审批请求</h1>
          <p className="header-subtitle">AI Agent 需要您的授权才能执行此操作</p>
        </div>

        {/* Content */}
        <div className="dialog-content">
          <div className="warning-message">
            <span className="warning-icon">⚡</span>
            AI Agent 正在尝试执行危险操作
          </div>

          <div className="operation-display">
            <h2 className="operation-name">{request.operation_name}</h2>
            <code className="operation-target">{request.target}</code>
          </div>

          <div className="details-section">
            <h3>操作详情</h3>

            <div className="detail-row">
              <span className="detail-label">操作类型:</span>
              <span className="detail-value">{request.operation}</span>
            </div>

            <div className="detail-row">
              <span className="detail-label">操作目标:</span>
              <span className="detail-value detail-target">{request.target}</span>
            </div>

            <div className="detail-row">
              <span className="detail-label">危险等级:</span>
              <span className={`detail-value risk-badge ${getRiskLevelClass(request.risk_level)}`}>
                {request.risk_level}
              </span>
            </div>

            <div className="detail-row">
              <span className="detail-label">原因:</span>
              <span className="detail-value">{request.reason}</span>
            </div>
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
                  width: `${(countdown / request.timeout_seconds) * 100}%`,
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
      </div>
    </div>
  )
}

export default ApprovalDialog
