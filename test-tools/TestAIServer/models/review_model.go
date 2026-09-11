package models

import (
	"encoding/json"
	"strings"
	"time"
)

// TestAIReview — Swarm M4 批作业验收 agent 测试模型。
//
// 供 in_review 自动验收链路的集成测试使用：对任意输入返回固定合法的验收
// JSON（verdict 由输入锚点决定），覆盖 G6 三态处置：
//   - 默认：PASS（auto_accept=false 语义下只出「待人工确认」意见）
//   - <REVIEW_FAIL>（埋在验收标准里，随 prompt 原样进评审输入）：
//     FAIL + gap 原文回显锚点——驱动 FAIL→重派→保险丝链路
//   - <REVIEW_UNSURE>：UNSURE——驱动转人工路径
//   - <REVIEW_BAD>：纯文本（无 JSON 对象）→ 解析失败 → 调用方按
//     UNSURE 诚实处置（回灌重试输入仍含 <REVIEW_BAD> → 3 轮全败）
//   - <REVIEW_EXP>（M4.5）：PASS + experience 槽位带固定经验对象
//     （pitfall / t29auth / "T29 经验锚点…"）——驱动验收蒸馏入库链路
//   - <SELFCHK_EVIDENCE_OK>（P4 B2b，二段验收）：必须先于
//     <REVIEW_NEED_EVIDENCE> 判定——二段输入同时携带原验收标准（含
//     <REVIEW_NEED_EVIDENCE>）与取证回报证据（含本锚点），先命中即 PASS，
//     驱动「取证回报 → 二段验收 → 自动收货」闭环
//   - <REVIEW_NEED_EVIDENCE>（P4 B2b，一段验收）：UNSURE +
//     need_evidence=true + evidence_request 指示 worker 回报时携带
//     <SELFCHK_EVIDENCE_OK>——驱动「验收暂缓 → 向执行节点取证」挂起链路
//
// 回灌重试提示词含「修正后重新输出」+原输出；FAIL/UNSURE 分支靠锚点
// 稳定复现，不受重试影响。确定性输出，零随机、零延迟。
type TestAIReview struct{}

type reviewOutput struct {
	Verdict    string   `json:"verdict"`
	Reasons    []string `json:"reasons"`
	Gap        string   `json:"gap"`
	Experience *struct {
		Category string `json:"category"`
		Scope    string `json:"scope"`
		Content  string `json:"content"`
	} `json:"experience"`
	NeedEvidence    *bool  `json:"need_evidence,omitempty"`
	EvidenceRequest string `json:"evidence_request,omitempty"`
}

func NewTestAIReview() *TestAIReview { return &TestAIReview{} }

func (m *TestAIReview) Name() string { return "testai-review-1.0" }

func (m *TestAIReview) Process(messages []Message) string {
	input := reviewInputText(messages)

	// P4 B2b 二段验收：取证回报证据带 <SELFCHK_EVIDENCE_OK> → 证据成立
	// 判 PASS。必须排在 <REVIEW_NEED_EVIDENCE> 之前——二段输入同时含两个
	// 锚点（原验收标准 + 证据），先命中证据锚点才能定案。
	if strings.Contains(input, "<SELFCHK_EVIDENCE_OK>") {
		return reviewMarshal(reviewOutput{
			Verdict: "PASS",
			Reasons: []string{"执行节点取证回报与验收标准逐项对照成立（二段验收，锚点 SELFCHK_EVIDENCE_OK）"},
			Gap:     "",
		})
	}

	switch {
	case strings.Contains(input, "<REVIEW_BAD>"):
		// 非 JSON 输出 → parse_review 首轮失败；重试提示词携带原输出
		//（不含新锚点）但输入仍含 <REVIEW_BAD> → 3 轮全败。
		return "抱歉，我暂时无法给出结构化验收意见。"
	case strings.Contains(input, "<REVIEW_EXP>"):
		// M4.5 蒸馏链路：PASS + 经验槽位（scope 固定 t29auth——T29 派发
		// 注入的检索键，任务文本含同名词即命中）。
		return reviewMarshal(reviewOutput{
			Verdict: "PASS",
			Reasons: []string{"自检结果与交付物清单对照验收标准成立"},
			Gap:     "",
			Experience: &struct {
				Category string `json:"category"`
				Scope    string `json:"scope"`
				Content  string `json:"content"`
			}{
				Category: "pitfall",
				Scope:    "t29auth",
				Content:  "T29 经验锚点：auth 会话 token 过期必须先刷新再重试，直接重发会被网关拒绝。",
			},
		})
	case strings.Contains(input, "<REVIEW_UNSURE>"):
		return reviewMarshal(reviewOutput{
			Verdict: "UNSURE",
			Reasons: []string{"验收标准无法客观判定（测试锚点 REVIEW_UNSURE）"},
			Gap:     "",
		})
	case strings.Contains(input, "<REVIEW_NEED_EVIDENCE>"):
		// P4 B2b 一段验收：证据不足挂起——need_evidence + 取证请求指示
		// worker 回报携带 <SELFCHK_EVIDENCE_OK>（命中上方二段 PASS 分支）。
		need := true
		return reviewMarshal(reviewOutput{
			Verdict: "UNSURE",
			Reasons: []string{"验收标准无法从交付物清单客观判定，需要执行节点补充证据（测试锚点 REVIEW_NEED_EVIDENCE）"},
			Gap:     "",
			NeedEvidence:    &need,
			EvidenceRequest: "逐项回报交付落实情况与关键输出原文，回报末尾附带标记 <SELFCHK_EVIDENCE_OK> 表示取证完成。",
		})
	case strings.Contains(input, "<REVIEW_FAIL>"):
		return reviewMarshal(reviewOutput{
			Verdict: "FAIL",
			Reasons: []string{"交付物清单与验收标准对不上（测试锚点 REVIEW_FAIL）"},
			Gap:     "验收标准要求的内容未出现在交付物清单：T28 差距锚点",
		})
	default:
		return reviewMarshal(reviewOutput{
			Verdict: "PASS",
			Reasons: []string{"自检结果与交付物清单对照验收标准成立"},
			Gap:     "",
		})
	}
}

func (m *TestAIReview) Delay() time.Duration { return 0 }

// reviewInputText 拼接全部消息文本（评审请求是单用户消息或回灌重试单
// 消息，全量扫描足够确定性）。
func reviewInputText(messages []Message) string {
	var buf strings.Builder
	for _, msg := range messages {
		buf.WriteString(msg.Content)
		buf.WriteString("\n")
	}
	return buf.String()
}

// reviewMarshal 序列化验收输出（json.Marshal 保证恒为合法 JSON 对象文本）。
func reviewMarshal(out reviewOutput) string {
	data, err := json.Marshal(out)
	if err != nil {
		// 结构体序列化不会失败；防御性兜底仍返回可被 parse_review 拒绝的文本。
		return "review 序列化失败"
	}
	return string(data)
}
