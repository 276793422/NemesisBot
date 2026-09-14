package models

import (
	"encoding/json"
	"strings"
	"time"
)

// TestAIConflictSolver — P5 冲突漏斗 AI 硬解桩（看板项目档案 goal P5）。
//
// 模拟 master 侧 conflict_resolver.rs 的 detached LLM 调用（同模型双职能：
// 硬解窗口内验收评审照常——master 切到本模型后，评审通道请求由内嵌
// TestAIReview 兜底应答，模型切换窗口内评审不炸）。
//
// 分支（扫描全部消息文本）：
//   - `<CONFLICT_SOLVE_BAD>`：恒败桩——返回纯文本（无 JSON）→ parse_resolutions
//     首轮失败；3 轮重试输入仍含标记 → 全败（驱动 T-mrg-4/7 硬解失败→重派/
//     预算转人工链路）
//   - `## 冲突文件`（conflict_resolver 提示词段落签名）：解析 `### <path>`
//     块 → 逐文件产出处置：
//       · 块内含「二进制文件」→ action=theirs（择边，不发明内容）
//       · 否则 → action=merge + 确定性合并内容 `resolved-by-ai-stub: <path>`
//     reason 恒含「建议重新生成」（锁文件理由闸放行任意文件集，T-mrg-6）
//   - 其余（评审提示词等）→ 委托 TestAIReview（默认 PASS 分支可收）
//
// 确定性输出，零随机；Delay=4s 放大硬解 3 轮失败窗口（T-mrg-5：冲突处理
// 与 t0 接触探针之间留出 kill 原 worker 的操作窗口——评审通道同延迟，仅
// 拖慢不计正确性）。
type TestAIConflictSolver struct {
	review *TestAIReview
}

func NewTestAIConflictSolver() *TestAIConflictSolver {
	return &TestAIConflictSolver{review: NewTestAIReview()}
}

func (m *TestAIConflictSolver) Name() string { return "testai-conflict-solver-1.0" }

func (m *TestAIConflictSolver) Process(messages []Message) string {
	input := solverInputText(messages)

	// 恒败桩：纯文本无 JSON → parse_resolutions 全轮失败（机械失败路径）。
	if strings.Contains(input, "<CONFLICT_SOLVE_BAD>") {
		return "抱歉，这个冲突我解决不了（测试桩恒败形态）。"
	}

	// 硬解分支：冲突段在场 → 产出全覆盖 resolutions JSON。
	if strings.Contains(input, "## 冲突文件") {
		return solverMarshal(solverBuildResolutions(input))
	}

	// 评审通道兜底（同模型双职能：模型切换窗口内的验收请求照常出合法 JSON）。
	return m.review.Process(messages)
}

func (m *TestAIConflictSolver) Delay() time.Duration { return 4 * time.Second }

type solverResolution struct {
	Path    string `json:"path"`
	Action  string `json:"action"`
	Content string `json:"content,omitempty"`
	Reason  string `json:"reason"`
}

// solverInputText 拼接全部消息文本（硬解请求是 system+user 单轮形态，全量
// 扫描足够确定性）。
func solverInputText(messages []Message) string {
	var buf strings.Builder
	for _, msg := range messages {
		buf.WriteString(msg.Content)
		buf.WriteString("\n")
	}
	return buf.String()
}

// solverBuildResolutions 解析 conflict_resolver 提示词的冲突清单并产出
// 全覆盖处置。「### <path>」块从该行起到下一个「### 」/「## 」/文末。
// 恒无覆盖缺口（parse_resolutions 的完备闸必过）。
func solverBuildResolutions(input string) []solverResolution {
	i := strings.Index(input, "## 冲突文件")
	if i < 0 {
		return nil
	}
	section := input[i:]
	// 逐块扫描：定位每个 "### " 行头。
	var out []solverResolution
	for {
		h := strings.Index(section, "\n### ")
		start := 0
		if strings.HasPrefix(section, "### ") {
			// 段首即是第一块。
		} else if h < 0 {
			break
		} else {
			start = h + 1 // 跳过换行，指向 "### "
		}
		rest := section[start+len("### "):]
		end := strings.Index(rest, "\n### ")
		if e2 := strings.Index(rest, "\n## "); e2 >= 0 && (end < 0 || e2 < end) {
			end = e2
		}
		var block string
		if end < 0 {
			block = rest
			section = ""
		} else {
			block = rest[:end]
			section = rest[end+1:]
		}
		path := strings.TrimSpace(strings.SplitN(block, "\n", 2)[0])
		if path == "" {
			continue
		}
		if strings.Contains(block, "二进制文件") {
			// E5/F6：二进制不进行级合并 → 择边对方（worker 变更集完整在场）。
			out = append(out, solverResolution{
				Path:   path,
				Action: "theirs",
				Reason: "测试桩确定性择边对方（二进制不发明内容）；若为锁文件建议重新生成以保证一致性",
			})
			continue
		}
		out = append(out, solverResolution{
			Path:    path,
			Action:  "merge",
			Content: "resolved-by-ai-stub: " + path + "\n",
			Reason:  "测试桩确定性落定；若为锁文件建议重新生成以保证一致性",
		})
	}
	return out
}

// solverMarshal 序列化产出（json.Marshal 保证恒为合法 JSON 对象文本）。
func solverMarshal(res []solverResolution) string {
	data, err := json.Marshal(map[string]any{"resolutions": res})
	if err != nil {
		return "conflict solver 序列化失败"
	}
	return string(data)
}
