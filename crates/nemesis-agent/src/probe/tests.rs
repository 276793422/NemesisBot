use super::*;
use crate::types::ToolCallInfo;

fn resp_with_tool(name: &str, args: &str) -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn read_file_task() -> ProbeTask {
    probe_tasks()
        .into_iter()
        .find(|t| t.expected_tool == "read_file")
        .unwrap()
}

#[test]
fn score_no_tool_call_is_all_zero() {
    let resp = LlmResponse {
        content: "I refuse to use tools.".to_string(),
        tool_calls: vec![],
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s, ProbeScore::default());
}

#[test]
fn score_correct_tool_valid_args_is_full_marks() {
    let resp = resp_with_tool("read_file", r#"{"path":"README.md"}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(
        s,
        ProbeScore {
            format: 1.0,
            selection: 1.0,
            schema: 1.0
        }
    );
}

#[test]
fn score_wrong_tool_is_zero_selection() {
    let resp = resp_with_tool("exec", r#"{"command":"cat README.md"}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s.selection, 0.0);
    assert_eq!(s.format, 1.0); // still used the channel
}

#[test]
fn score_autofixable_args_is_half_schema() {
    // "patch" is edit-distance 1 from "path" → autofixed → 0.5
    let resp = resp_with_tool("read_file", r#"{"patch":"README.md"}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s.schema, 0.5);
    assert_eq!(s.selection, 1.0);
}

#[test]
fn score_missing_required_is_zero_schema() {
    let resp = resp_with_tool("read_file", r#"{}"#);
    let s = score_response(&resp, &read_file_task());
    assert_eq!(s.schema, 0.0);
}

#[test]
fn tier_mapping() {
    assert_eq!(tier_from_scores(1.0, 1.0, 1.0), ModelTier::Big);
    assert_eq!(tier_from_scores(0.9, 0.85, 0.7), ModelTier::Normal);
    assert_eq!(tier_from_scores(0.3, 0.3, 0.3), ModelTier::Mini);
    assert_eq!(tier_from_scores(0.0, 0.0, 0.0), ModelTier::Mini);
}

#[test]
fn probe_tasks_has_seven_including_cluster() {
    let tasks = probe_tasks();
    assert_eq!(tasks.len(), 7);
    assert!(tasks.iter().any(|t| t.expected_tool == "cluster_rpc"));
}

// --- T10（多模态 D9）：第 8 题视觉探针 ---

/// 视觉探针载荷必须是合法 PNG：签名 + chunk 结构/CRC 全对 + IHDR 1×1 + IDAT
/// 存在（防手抄/替换出错——探针请求真的发这张图；zlib 解压正确性由外部
/// 工具离线验证过，这里不重复实现 inflate）。
#[test]
fn vision_probe_payload_is_valid_png() {
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD
        .decode(VISION_PROBE_PNG_B64)
        .expect("payload must be valid base64");
    assert_eq!(
        &data[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "PNG signature"
    );
    let mut i = 8usize;
    let mut saw_ihdr = false;
    let mut saw_idat = false;
    while i < data.len() {
        let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        assert!(
            i + 12 + len <= data.len(),
            "chunk bounds must stay inside the payload"
        );
        let ctype = &data[i + 4..i + 8];
        let chunk = &data[i + 8..i + 8 + len];
        let crc = u32::from_be_bytes([
            data[i + 8 + len],
            data[i + 9 + len],
            data[i + 10 + len],
            data[i + 11 + len],
        ]);
        assert_eq!(
            crc,
            crc32(ctype, chunk),
            "chunk {:?} CRC must match",
            std::str::from_utf8(ctype).unwrap_or("?")
        );
        match ctype {
            b"IHDR" => {
                saw_ihdr = true;
                assert_eq!(len, 13);
                assert_eq!(&chunk[..8], &[0, 0, 0, 1, 0, 0, 0, 1], "1×1");
            }
            b"IDAT" => {
                saw_idat = true;
                assert!(!chunk.is_empty());
            }
            b"IEND" => assert_eq!(len, 0),
            _ => panic!("unexpected chunk {:?}", ctype),
        }
        i += 12 + len;
    }
    assert!(saw_ihdr, "IHDR required");
    assert!(saw_idat, "IDAT required");
}

/// 最小 CRC32（zlib 多项式，逐位实现——测试内自足，不引依赖）。
fn crc32(ctype: &[u8], chunk: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for b in ctype.iter().chain(chunk.iter()) {
        crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

#[test]
fn probe_tool_defs_dedupes() {
    let defs = probe_tool_defs();
    // 7 tasks but several share read_file/write_file/etc tools; dedup by name.
    let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(names.len(), sorted.len()); // no dupes
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"cluster_rpc"));
}

// --- W3a: run() 电池测试（mock provider 按 prompt 分发）---

use crate::types::{ChatOptions, ToolDefinition};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 每个探针任务的合法参数（schema Valid）。
fn valid_args_for(tool: &str) -> &'static str {
    match tool {
        "exec" => r#"{"command":"date"}"#,
        "read_file" => r#"{"path":"README.md"}"#,
        "create_dir" => r#"{"path":"test"}"#,
        "grep" => r#"{"pattern":"TODO"}"#,
        "write_file" => r#"{"path":"note.md","content":"x"}"#,
        "edit_file" => r#"{"path":"note.md","old_text":"foo","new_text":"bar"}"#,
        "cluster_rpc" => r#"{"target_node":"n1","message":"你好"}"#,
        _ => r#"{}"#,
    }
}

fn empty_resp() -> LlmResponse {
    LlmResponse {
        content: "I'll just answer directly.".to_string(),
        tool_calls: vec![],
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 按 user prompt 找到当前任务，再按 mode 生成响应。带图消息（T10 第 8 题
/// 视觉探针）优先于电池 prompt 匹配——视觉请求没有工具上下文，走独立分支。
struct MockProbeProvider {
    mode: &'static str, // "perfect" | "normal" | "mini" | "fail" | "vision_reject" | "vision_transport"
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::r#loop::LlmProvider for MockProbeProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<crate::r#loop::LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.mode == "fail" {
            return Err("provider down".to_string());
        }
        // T10：视觉探针请求（带图、无工具）——结果只取决于 mode。
        if messages.iter().any(|m| !m.images.is_empty()) {
            return match self.mode {
                "vision_transport" => Err("error sending request: connection refused".to_string()),
                // 非传输类拒绝（provider 4xx 形态）→ 探针应判 Some(false)。
                "vision_reject" => Err("400 Bad Request: image input not supported".to_string()),
                "fail" => unreachable!(),
                _ => Ok(LlmResponse {
                    content: "灰色".to_string(),
                    tool_calls: vec![],
                    finished: true,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                }),
            };
        }
        let prompt = messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let task = probe_tasks()
            .into_iter()
            .find(|t| t.prompt == prompt.as_str())
            .expect("prompt must match a probe task");

        if self.mode == "vision_reject" || self.mode == "vision_transport" {
            // 工具电池照常满分；只有第 8 题视觉探针失败（模式各自决定失败形态）。
            return Ok(resp_with_tool(
                task.expected_tool,
                valid_args_for(task.expected_tool),
            ));
        }

        match self.mode {
            "perfect" => Ok(resp_with_tool(
                task.expected_tool,
                valid_args_for(task.expected_tool),
            )),
            "normal" => {
                if task.expected_tool == "edit_file" {
                    Ok(empty_resp()) // format 失败一题
                } else if task.expected_tool == "grep" {
                    // 用了工具通道但选错工具（args 对 grep 任务仍合法）
                    Ok(resp_with_tool("exec", valid_args_for("grep")))
                } else {
                    Ok(resp_with_tool(
                        task.expected_tool,
                        valid_args_for(task.expected_tool),
                    ))
                }
            }
            "mini" => {
                // 前 4 题完美，后 3 题不使用工具 → 三轴 4/7 ≈ 0.571 → Mini
                match task.expected_tool {
                    "exec" | "read_file" | "create_dir" | "grep" => Ok(resp_with_tool(
                        task.expected_tool,
                        valid_args_for(task.expected_tool),
                    )),
                    _ => Ok(empty_resp()),
                }
            }
            _ => unreachable!("unknown mode"),
        }
    }
}

#[tokio::test]
async fn run_all_perfect_maps_to_big() {
    let p = MockProbeProvider {
        mode: "perfect",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    assert_eq!(report.format_score, 1.0);
    assert_eq!(report.selection_score, 1.0);
    assert_eq!(report.schema_score, 1.0);
    assert_eq!(report.tier, ModelTier::Big);
    assert_eq!(report.per_task.len(), 7);
    assert_eq!(report.per_task[0].0, "exec");
    assert!(report.per_task.iter().all(|(_, s)| s.format == 1.0));
    assert_eq!(
        p.calls.load(Ordering::SeqCst),
        8,
        "7 tool tasks + 1 vision probe"
    );
    // T10 第 8 题：带图请求成功 → Some(true)。
    assert_eq!(report.vision_probe, Some(true));
}

#[tokio::test]
async fn run_vision_reject_maps_to_some_false() {
    // 工具电池满分，但第 8 题模型拒绝带图请求（非传输类 4xx）→ Some(false)。
    let p = MockProbeProvider {
        mode: "vision_reject",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    assert_eq!(report.tier, ModelTier::Big, "tier 打分不受第 8 题影响");
    assert_eq!(report.vision_probe, Some(false));
}

#[tokio::test]
async fn run_vision_transport_error_is_inconclusive() {
    // 传输类失败（connection refused）不说明模型不支持视觉 → None（未定）。
    let p = MockProbeProvider {
        mode: "vision_transport",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    assert_eq!(report.vision_probe, None);
}

#[tokio::test]
async fn run_mixed_performance_maps_to_normal() {
    // fmt=6/7≈0.857 sel=5/7≈0.714 schema=6/7≈0.857 → 不满足 Big(sel<0.8)，
    // 满足 Normal(schema>=0.6 且 fmt/sel>=0.5)。
    let p = MockProbeProvider {
        mode: "normal",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    assert!(report.format_score > 0.8 && report.format_score < 1.0);
    assert!(report.selection_score < 0.8);
    assert_eq!(report.tier, ModelTier::Normal);
}

#[tokio::test]
async fn run_majority_format_failure_maps_to_mini() {
    // 4/7 三轴 ≈ 0.571：Big 不满足，Normal 的 schema>=0.6 不满足 → Mini。
    let p = MockProbeProvider {
        mode: "mini",
        calls: AtomicUsize::new(0),
    };
    let report = run(&p, "test-model").await.expect("probe run ok");
    let expected = 4.0 / 7.0;
    assert!((report.format_score - expected).abs() < 1e-9);
    assert!((report.selection_score - expected).abs() < 1e-9);
    assert_eq!(report.tier, ModelTier::Mini);
}

#[tokio::test]
async fn run_provider_error_propagates_with_task_name() {
    let p = MockProbeProvider {
        mode: "fail",
        calls: AtomicUsize::new(0),
    };
    let err = run(&p, "test-model").await.expect_err("must fail");
    assert!(err.contains("LLM chat failed"), "err: {err}");
    assert!(err.contains("exec"), "first task name in err: {err}");
    assert!(err.contains("provider down"), "err: {err}");
    assert_eq!(p.calls.load(Ordering::SeqCst), 1, "fails on first task");
}

// --- F-G（2026-09-04 四轮盲审）：瞬态错误的 FailoverError Display 形态
// 必须命中传输标记表（误判 Some(false) 会永久钉死 vision_probe）---

#[test]
fn fg_failover_error_display_forms_are_transport_markers() {
    // nemesis-providers failover.rs 的真实 Display 渲染（限流/过载均不
    // 含原始数字码——旧表只匹配 "502"/"503" 数字漏接）。
    let rate_limited = format!("rate limited by provider {}/{}", "p", "m");
    let overloaded = format!("provider {} is overloaded", "p");
    for err in [
        rate_limited.as_str(),
        overloaded.as_str(),
        "HTTP 429 too many requests",
    ] {
        let lower = err.to_lowercase();
        assert!(
            VISION_PROBE_TRANSPORT_MARKERS
                .iter()
                .any(|m| lower.contains(m)),
            "瞬态错误必须判为传输类（探针未定，不得写 false 钉死）: {}",
            err
        );
    }
}

/// 反向锚定：真正的"模型拒绝图像"错误**不**在新标记里（探针仍能判 false）。
#[test]
fn fg_model_rejection_errors_still_not_transport() {
    for err in [
        "invalid request: image input not supported by this model",
        "400 bad request: unknown field images",
    ] {
        let lower = err.to_lowercase();
        assert!(
            !VISION_PROBE_TRANSPORT_MARKERS
                .iter()
                .any(|m| lower.contains(m)),
            "模型拒绝类错误不应命中传输标记: {}",
            err
        );
    }
}
