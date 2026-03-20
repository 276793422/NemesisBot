//go:build !multi_process

package approval

import (
	"context"
	"fmt"
	"log"
	"time"
)

// RequestApproval 请求用户审批（单进程实现）
//
// 这是核心方法，会弹出模态对话框征求用户同意
// 参数:
//   - ctx: 上下文，用于取消操作
//   - req: 审批请求，包含操作详情
// 返回:
//   - *ApprovalResponse: 用户响应结果
//   - error: 请求失败时返回错误
func (m *approvalManagerImpl) RequestApproval(ctx context.Context, req *ApprovalRequest) (*ApprovalResponse, error) {
	if !m.running {
		return nil, fmt.Errorf("approval manager is not running")
	}

	log.Printf("[approval] Requesting approval: request_id=%s, operation=%s, target=%s, risk_level=%s",
		req.RequestID,
		req.Operation,
		req.Target,
		req.RiskLevel,
	)

	startTime := time.Now()

	// 创建 WebView（使用适配器，自动选择平台实现）
	adapter := NewWebViewAdapter()
	defer adapter.Destroy()

	// 创建窗口（平台无关）
	err := adapter.Create("Security Approval - NemesisBot", m.config.DialogWidth, m.config.DialogHeight)
	if err != nil {
		return nil, fmt.Errorf("failed to create webview: %w", err)
	}

	// 设置 HTML（平台无关）
	html := getDialogHTML()
	if err := adapter.SetHTML(html); err != nil {
		return nil, fmt.Errorf("failed to set HTML: %w", err)
	}

	// 创建响应通道
	responseChan := make(chan *ApprovalResponse, 1)

	// 绑定响应函数（平台无关）
	if err := adapter.Bind("sendApprovalResponse", func(approved bool) {
		select {
		case responseChan <- &ApprovalResponse{
			RequestID:    req.RequestID,
			Approved:     approved,
			TimedOut:     false,
			ResponseTime: time.Now().Unix(),
		}:
		default:
			// 避免重复发送
		}
	}); err != nil {
		return nil, fmt.Errorf("failed to bind function: %w", err)
	}

	// 初始化对话框数据（平台无关）
	initJS := fmt.Sprintf(`
		window.approvalData = {
			requestId: "%s",
			operation: "%s",
			operationName: %s,
			target: %s,
			riskLevel: "%s",
			reason: %s,
			timeoutSeconds: %d
		};
		if (window.initApp) {
			window.initApp();
		}
	`,
		jsEscape(req.RequestID),
		jsEscape(req.Operation),
		jsEscape(GetOperationDisplayName(req.Operation)),
		jsEscape(req.Target),
		req.RiskLevel,
		jsEscape(req.Reason),
		req.TimeoutSeconds,
	)

	if err := adapter.Eval(initJS); err != nil {
		return nil, fmt.Errorf("failed to eval init JS: %w", err)
	}

	// 启动超时定时器
	timeout := time.Duration(req.TimeoutSeconds) * time.Second
	timeoutChan := make(chan bool, 1)

	go func() {
		time.Sleep(timeout)
		timeoutChan <- true
	}()

	// 启动 WebView（阻塞，在主线程运行）
	// 注意：WebView2 必须在主线程调用 Run()
	go func() {
		// Run() 会阻塞直到 Terminate() 被调用
		adapter.Run()
	}()

	// 等待响应或超时
	select {
	case resp := <-responseChan:
		resp.DurationSeconds = time.Since(startTime).Seconds()
		adapter.Terminate()

		log.Printf("[approval] Approval received: request_id=%s, approved=%v, duration=%f",
			resp.RequestID,
			resp.Approved,
			resp.DurationSeconds,
		)

		return resp, nil

	case <-timeoutChan:
		adapter.Terminate()

		resp := &ApprovalResponse{
			RequestID:       req.RequestID,
			Approved:        false,
			TimedOut:        true,
			DurationSeconds: time.Since(startTime).Seconds(),
			ResponseTime:    time.Now().Unix(),
		}

		log.Printf("[approval] Approval timed out: request_id=%s, duration=%f",
			resp.RequestID,
			resp.DurationSeconds,
		)

		return resp, nil

	case <-ctx.Done():
		adapter.Terminate()
		return nil, ctx.Err()
	}
}

// jsEscape 转义 JavaScript 字符串
func jsEscape(s string) string {
	// 简单的 JSON 编码转义
	if s == "" {
	return "\"\""
	}
	return fmt.Sprintf("%q", s)
}

// getDialogHTML 获取对话框 HTML 内容
func getDialogHTML() string {
	return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Security Approval</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        html, body {
            height: 100%;
            overflow: hidden;
        }

        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Arial, sans-serif;
            background: linear-gradient(135deg, #1a1a1a 0%, #0d0d0d 100%);
            color: #ffffff;
            display: flex;
            align-items: center;
            justify-content: center;
            height: 100%;
        }

        .container {
            background: linear-gradient(135deg, #2d2d2d 0%, #1a1a1a 100%);
            border: 1px solid #3a3a3a;
            border-radius: 12px;
            box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
            padding: 16px 20px;
            width: calc(100% - 40px);
            max-width: 520px;
            display: flex;
            flex-direction: column;
            gap: 10px;
            animation: slideIn 0.3s ease-out;
        }

        @keyframes slideIn {
            from {
                transform: translateY(-20px);
                opacity: 0;
            }
            to {
                transform: translateY(0);
                opacity: 1;
            }
        }

        .header {
            text-align: center;
        }

        .header h1 {
            font-size: 16px;
            margin: 0;
            display: flex;
            align-items: center;
            justify-content: center;
            gap: 6px;
        }

        .message {
            color: #b0b0b0;
            text-align: center;
            font-size: 11px;
            margin-top: 2px;
        }

        .operation-name {
            color: #ff6b6b;
            text-align: center;
            font-size: 13px;
            font-weight: 600;
            padding: 8px 10px;
            background: rgba(255, 107, 107, 0.1);
            border-radius: 6px;
        }

        .details {
            background: #252525;
            border: 1px solid #3a3a3a;
            border-radius: 8px;
            padding: 10px 12px;
        }

        .details h2 {
            color: #ffffff;
            font-size: 10px;
            margin: 0 0 8px 0;
            text-transform: uppercase;
            letter-spacing: 0.5px;
        }

        .detail-row {
            display: flex;
            justify-content: space-between;
            padding: 5px 0;
            border-bottom: 1px solid #3a3a3a;
        }

        .detail-row:last-child {
            border-bottom: none;
            padding-bottom: 0;
        }

        .detail-label {
            color: #808080;
            font-size: 10px;
        }

        .detail-value {
            color: #ffffff;
            font-weight: 500;
            font-size: 10px;
            text-align: right;
            word-break: break-all;
            margin-left: 10px;
        }

        .risk-level {
            font-weight: 700;
            padding: 1px 5px;
            border-radius: 3px;
            font-size: 9px;
        }

        .risk-level.CRITICAL {
            color: #ff6b6b;
            background: rgba(255, 107, 107, 0.2);
        }

        .risk-level.HIGH {
            color: #ffa500;
            background: rgba(255, 165, 0, 0.2);
        }

        .risk-level.MEDIUM {
            color: #ffd700;
            background: rgba(255, 215, 0, 0.2);
        }

        .risk-level.LOW {
            color: #4ecdc4;
            background: rgba(78, 205, 196, 0.2);
        }

        .countdown {
            text-align: center;
            color: #ff6b6b;
            font-size: 12px;
            font-weight: 600;
            padding: 6px 10px;
            background: rgba(255, 107, 107, 0.1);
            border-radius: 6px;
            display: flex;
            align-items: center;
            justify-content: center;
            gap: 6px;
        }

        .countdown .icon {
            font-size: 13px;
        }

        .footer {
            display: flex;
            gap: 8px;
            justify-content: center;
            border-top: 1px solid #3a3a3a;
            padding-top: 10px;
        }

        .btn {
            flex: 1;
            padding: 8px 16px;
            border: none;
            border-radius: 5px;
            font-size: 12px;
            font-weight: 600;
            cursor: pointer;
            transition: all 0.2s;
            display: flex;
            align-items: center;
            justify-content: center;
            gap: 5px;
        }

        .btn:hover {
            transform: translateY(-1px);
            box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
        }

        .btn-allow {
            background: #4ecdc4;
            color: #1a1a1a;
        }

        .btn-allow:hover {
            background: #45b7aa;
        }

        .btn-deny {
            background: #ff6b6b;
            color: #ffffff;
        }

        .btn-deny:hover {
            background: #ee5a5a;
        }

        .btn .icon {
            font-size: 12px;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>
                <span style="font-size: 16px;">⚠️</span>
                <span style="font-size: 13px;">🔒</span>
            </h1>
        </div>

        <p class="message">AI Agent is attempting a dangerous operation:</p>

        <p class="operation-name" id="operation-name">Loading...</p>

        <div class="details">
            <h2>Operation Details</h2>

            <div class="detail-row">
                <span class="detail-label">Operation Type:</span>
                <span class="detail-value" id="operation-type">-</span>
            </div>

            <div class="detail-row">
                <span class="detail-label">Target:</span>
                <span class="detail-value" id="operation-target">-</span>
            </div>

            <div class="detail-row">
                <span class="detail-label">Risk Level:</span>
                <span class="detail-value risk-level" id="risk-level">-</span>
            </div>

            <div class="detail-row">
                <span class="detail-label">Reason:</span>
                <span class="detail-value" id="reason">-</span>
            </div>
        </div>

        <div class="countdown">
            <span class="icon">⏰</span>
            <span id="countdown-text">30</span>
            <span>seconds until auto-reject</span>
        </div>

        <div class="footer">
            <button class="btn btn-allow" id="btn-allow">
                <span class="icon">✓</span>
                <span>Allow</span>
            </button>

            <button class="btn btn-deny" id="btn-deny">
                <span class="icon">✗</span>
                <span>Deny</span>
            </button>
        </div>
    </div>

    <script>
        let countdownInterval = null;
        let timeoutSeconds = 30;

        window.initApp = function() {
            if (window.approvalData) {
                document.getElementById('operation-name').textContent =
                    window.approvalData.operationName || window.approvalData.operation;
                document.getElementById('operation-type').textContent =
                    window.approvalData.operation || '-';
                document.getElementById('operation-target').textContent =
                    window.approvalData.target || '-';
                document.getElementById('risk-level').textContent =
                    window.approvalData.riskLevel || 'UNKNOWN';

                const riskLevel = document.getElementById('risk-level');
                if (riskLevel) {
                    riskLevel.className = 'detail-value risk-level ' + window.approvalData.riskLevel;
                }

                document.getElementById('reason').textContent =
                    window.approvalData.reason || '-';

                timeoutSeconds = window.approvalData.timeoutSeconds || 30;

                startCountdown();
            }

            document.getElementById('btn-allow').addEventListener('click', handleAllow);
            document.getElementById('btn-deny').addEventListener('click', handleDeny);
        };

        function startCountdown() {
            stopCountdown();
            let seconds = timeoutSeconds;
            updateCountdown(seconds);

            countdownInterval = setInterval(function() {
                seconds--;
                updateCountdown(seconds);

                if (seconds <= 0) {
                    stopCountdown();
                    handleTimeout();
                }
            }, 1000);
        }

        function stopCountdown() {
            if (countdownInterval) {
                clearInterval(countdownInterval);
                countdownInterval = null;
            }
        }

        function updateCountdown(seconds) {
            document.getElementById('countdown-text').textContent = seconds;
        }

        function handleAllow() {
            stopCountdown();
            window.sendApprovalResponse(true);
        }

        function handleDeny() {
            stopCountdown();
            window.sendApprovalResponse(false);
        }

        function handleTimeout() {
            stopCountdown();
            window.sendApprovalResponse(false);
        }

        window.sendApprovalResponse = function(approved) {
            console.log('Sending response to Go:', approved);
        };
    </script>
</body>
</html>`
}
