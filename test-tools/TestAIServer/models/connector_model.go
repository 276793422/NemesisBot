package models

import (
	"encoding/json"
	"strings"
	"time"
)

// TestAIConn — connector 注册表 e2e 桩（确定性 marker 状态机）。
//
// 每轮用户消息携带一个 CONN 标记，模型返回对应的**真实工具调用**（由
// e2e 在 gateway 侧断言工具面行为）；工具结果回灌轮（last role=tool）
// 不做特殊处理——直接给终端文本收口，下一轮用户消息的新标记驱动下一步。
// 标记匹配优先级：FIND > QUOTE > ORDER > APPLY > NOBUDGET（只看最后一条
// 用户消息——会话复用下早前轮的标记留在历史里，全历史扫描会重放旧动作）；
// <CONN_FINAL> 或最后一条用户消息不含任何 CONN 标记 → 纯文本
// CONN_DEMO_DONE（固定字符串，供 e2e 断言）。
//
//	<CONN_FIND>     → connector_find {"query": ""}（列全部 connector）
//	<CONN_QUOTE>    → connector_invoke paper-broker/quote
//	                  {"symbol": "AAPL"}
//	<CONN_ORDER>    → connector_invoke paper-broker/place_order
//	                  {"side": "buy", "symbol": "AAPL", "qty": 10, "price": 150.0}
//	<CONN_APPLY>    → connector_invoke local-fs/move
//	                  {"command": "echo demo"}
//	<CONN_NOBUDGET> → connector_invoke paper-broker/place_order
//	                  {"side": "buy", "symbol": "MSFT", "qty": 999999999, "price": 500.0}
//	                  （超资金限额拒绝路径）
//	<CONN_REMOTE>   → connector_invoke paper-broker/positions + target_node=Node-B
//	                  （收官批 E1 集群分发：read/low 零闸 op 干净走远程执行面）
//	<CONN_CRED>     → connector_invoke cred-proof/ping + target_node=Node-B
//	                  （凭据不上线证明：manifest 声明 required 凭据，worker
//	                  用自己的 vault 解锁——master 永不持有真值）
//
// price 用 json.Number 字面量（"150.0"/"500.0"）：json.Marshal(float64)
// 会把 150.0 压成 150，serde_json 的 Number 相等比较区分整数/浮点形态，
// e2e 若按 json!(150.0) 做严格 Value 相等断言会假红。
type TestAIConn struct{}

func NewTestAIConn() *TestAIConn { return &TestAIConn{} }

func (m *TestAIConn) Name() string { return "testai-conn-1.0" }

func (m *TestAIConn) Delay() time.Duration { return 0 }

// connDone 终端回复：工具结果回灌轮与 <CONN_FINAL>/无标记轮共用，
// e2e 以此固定字符串断言回合收口。
const connDone = "CONN_DEMO_DONE"

func (m *TestAIConn) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]

	// 工具结果回灌轮：无需特殊处理，终端文本收口。
	if last.Role == "tool" {
		return connDone
	}

	if last.Role == "user" {
		c := last.Content
		switch {
		case strings.Contains(c, "<CONN_FIND>"):
			return buildSingleToolCall("connector_find", map[string]interface{}{
				"query": "",
			})
		case strings.Contains(c, "<CONN_QUOTE>"):
			return buildSingleToolCall("connector_invoke", map[string]interface{}{
				"connector": "paper-broker",
				"op":        "quote",
				"args": map[string]interface{}{
					"symbol": "AAPL",
				},
			})
		case strings.Contains(c, "<CONN_ORDER>"):
			return buildSingleToolCall("connector_invoke", map[string]interface{}{
				"connector": "paper-broker",
				"op":        "place_order",
				"args": map[string]interface{}{
					"side":   "buy",
					"symbol": "AAPL",
					"qty":    10,
					"price":  json.Number("150.0"),
				},
			})
		case strings.Contains(c, "<CONN_APPLY>"):
			return buildSingleToolCall("connector_invoke", map[string]interface{}{
				"connector": "local-fs",
				"op":        "move",
				"args": map[string]interface{}{
					"command": "echo demo",
				},
			})
		case strings.Contains(c, "<CONN_NOBUDGET>"):
			return buildSingleToolCall("connector_invoke", map[string]interface{}{
				"connector": "paper-broker",
				"op":        "place_order",
				"args": map[string]interface{}{
					"side":   "buy",
					"symbol": "MSFT",
					"qty":    999999999,
					"price":  json.Number("500.0"),
				},
			})
		case strings.Contains(c, "<CONN_REMOTE>"):
			return buildSingleToolCall("connector_invoke", map[string]interface{}{
				"connector":   "paper-broker",
				"op":          "positions",
				"target_node": "Node-B",
				"args":        map[string]interface{}{},
			})
		case strings.Contains(c, "<CONN_CRED>"):
			return buildSingleToolCall("connector_invoke", map[string]interface{}{
				"connector":   "cred-proof",
				"op":          "ping",
				"target_node": "Node-B",
				"args":        map[string]interface{}{},
			})
		}
	}

	// <CONN_FINAL> 或不含任何 CONN 标记 → 固定终端文本。
	return connDone
}
