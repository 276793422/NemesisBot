//! COV 覆盖率批次：状态注入 + 纯逻辑可达面（第五子模块）。聚焦——
//! DialogueSttOutput 缓冲/部分上屏/barge-in 标注与 Wrapper 委派、WsSttOutput
//! 默认 no-op 方法与广播投影、config.voice(.print).json RMW helper（含事务
//! 中止语义）、voice_shutdown 注入收口、dialogue stop/reset 与 tts_playback
//! stop 状态机、speaker 注册冲突/阈值持久化/status 投影、engine_status
//! dialogue-active 投影、dispatch 错误面、纯 FS helper 矩阵。
//! 真 ONNX 引擎 / sherpa DLL / 模型下载 / 真麦克风臂归结构性豁免
//! （台账 VE-voice-1/VE-voice-2 延续）。全局单例触碰一律持
//! s10_tests::voice_state_lock 同一把 crate 级锁。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::{AuthMethod, SessionManager};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn cov_lock() -> std::sync::MutexGuard<'static, ()> {
    super::s10_tests::voice_state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn collecting_push() -> (
    Box<dyn Fn(&str) + Send + Sync>,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink2 = sink.clone();
    (
        Box::new(move |s: &str| sink2.lock().unwrap().push(s.to_string())),
        sink,
    )
}

fn make_dialogue_output(push: Box<dyn Fn(&str) + Send + Sync>) -> Arc<DialogueSttOutput> {
    Arc::new(DialogueSttOutput {
        push_fn: push,
        state: Arc::new(std::sync::Mutex::new(DialogueState {
            buffer: String::new(),
            silence_timeout_secs: 3.0,
            reset_flag: false,
        })),
    })
}

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "cov".to_string(),
        chat_id: "ccov".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: AuthMethod::default(),
    }
}

// ── DialogueSttOutput 缓冲语义 ───────────────────────────────────────────

#[test]
fn dialogue_output_buffer_semantics() {
    let (push, sink) = collecting_push();
    let out = make_dialogue_output(push);

    // 空文本与状态行（[前缀）不入缓冲
    out.send_text("hello");
    out.send_text("   ");
    out.send_text("[听写已开始]");
    // partial 草稿只上屏不进缓冲；空 partial 静默
    out.send_partial("draft-one");
    out.send_partial("   ");
    out.notify_barge_in();

    let msgs = sink.lock().unwrap().clone();
    assert_eq!(
        msgs,
        vec![
            "accumulate:hello".to_string(),
            "partial:draft-one".to_string(),
            "barge_in:1".to_string(),
        ],
        "{msgs:?}"
    );

    // reset 清缓冲 + 置旗：下一句 send_text 先清残留再累积
    out.reset();
    out.send_text("next");
    let flush = out.flush().expect("缓冲非空");
    assert_eq!(flush, "next");
    assert!(out.flush().is_none(), "flush 后缓冲空 = None");

    // Wrapper 委派三方法
    let (push2, sink2) = collecting_push();
    let wrapper = DialogueSttOutputWrapper {
        inner: make_dialogue_output(push2),
    };
    SttOutput::send_text(&wrapper, "via-wrapper");
    SttOutput::send_partial(&wrapper, "wp");
    SttOutput::notify_barge_in(&wrapper);
    let msgs2 = sink2.lock().unwrap().clone();
    assert_eq!(
        msgs2,
        vec![
            "accumulate:via-wrapper".to_string(),
            "partial:wp".to_string(),
            "barge_in:1".to_string(),
        ],
        "{msgs2:?}"
    );
}

#[tokio::test]
async fn ws_output_broadcast_and_default_noop_methods() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    // 广播到不存在会话：spawn 内 warn，不 panic（send_text 投影臂）
    let ws_out = WsSttOutput {
        session_id: "ghost-session".to_string(),
        session_mgr: mgr.clone(),
    };
    SttOutput::send_text(&ws_out, "text-for-ghost");
    // 默认 trait 方法（未覆写）：no-op
    SttOutput::send_partial(&ws_out, "ignored");
    SttOutput::notify_barge_in(&ws_out);

    // 输入盒投影：push_fn 直通
    let (push, sink) = collecting_push();
    let input_out = InputBoxSttOutput { push_fn: push };
    SttOutput::send_text(&input_out, "to-input-box");
    assert_eq!(
        sink.lock().unwrap().last().map(String::as_str),
        Some("to-input-box")
    );
    // 默认方法同样静默
    SttOutput::send_partial(&input_out, "ignored");
    SttOutput::notify_barge_in(&input_out);

    // NoopTtsInput：listen 从不触发
    NoopTtsInput.listen(Box::new(|_: &str| panic!("no-op input must not fire")));
}

// ── 配置 RMW helper ──────────────────────────────────────────────────────

#[test]
fn voice_and_voiceprint_config_rmw_helpers() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("config");

    // 缺席种子：ensure 落默认文件，read 宽容回退
    let path = ensure_voice_config(&cfg_dir);
    assert!(path.exists(), "ensure 建默认 config.voice.json");
    let cfg = read_voice_config(&cfg_dir);
    assert!(cfg.is_object(), "默认配置可解析: {cfg}");
    assert!(path.exists(), "chat config 建默认文件");
    let chat = read_chat_config(&cfg_dir);
    assert!(chat.is_object(), "默认 chat 配置可解析: {chat}");

    // 声纹配置：默认（threshold/speakers 种子）→ RMW 写 → 读回
    let vp = read_voiceprint_config(&cfg_dir);
    assert_eq!(vp["threshold"], DEFAULT_SPEAKER_THRESHOLD as f64, "{vp}");
    assert!(vp["speakers"].is_object());
    update_voiceprint_config(&cfg_dir, |v| {
        v["threshold"] = serde_json::json!(0.42);
        v["speakers"]["alice"] = serde_json::json!({"embedding": [1.0]});
        Ok(())
    })
    .expect("声纹 RMW 落盘");
    let vp2 = read_voiceprint_config(&cfg_dir);
    assert_eq!(vp2["threshold"], 0.42);
    assert!(vp2["speakers"]["alice"].is_object());

    // 声纹写路径建父目录（config/ 缺席首写不裸失败）
    let lonely = tmp.path().join("lonely").join("config");
    write_voiceprint_config(
        &lonely,
        &serde_json::json!({"threshold": 0.5, "speakers": {}}),
    )
    .expect("首写自动建父目录");

    // voice config RMW：闭包 Err = abort（不落盘）
    let before = std::fs::read_to_string(ensure_voice_config(&cfg_dir)).unwrap();
    let err = update_voice_config::<()>(&cfg_dir, |_| Err("boom".to_string())).unwrap_err();
    assert_eq!(err, "boom");
    let after = std::fs::read_to_string(ensure_voice_config(&cfg_dir)).unwrap();
    assert_eq!(before, after, "闭包 Err 不得落盘");
    update_voice_config(&cfg_dir, |v| {
        v["speaker_id"] = serde_json::json!(77);
        Ok(())
    })
    .expect("voice RMW 落盘");
    let cfg2 = read_voice_config(&cfg_dir);
    assert_eq!(cfg2["speaker_id"], 77, "RMW 后读回新值");
}

// ── voice_shutdown 注入收口 ──────────────────────────────────────────────

#[tokio::test]
async fn voice_shutdown_cancels_all_injected_sessions() {
    let _guard = cov_lock();
    // 注入四类活动态 + 引擎槽恒空（释放臂对 Some 才动作——空槽幂等）
    let out = make_dialogue_output(Box::new(|_: &str| {}));
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: Some(out.clone()),
    });
    let reg_cancel = CancellationToken::new();
    *speaker_register_state().lock().unwrap() = Some(SpeakerRegistration {
        name: "owner".to_string(),
        samples: std::sync::Mutex::new(Vec::new()),
        sample_rate: 16000,
        start_time: Instant::now(),
        cancel: reg_cancel.clone(),
    });
    let test_cancel = CancellationToken::new();
    *speaker_test_state().lock().await = Some(SpeakerTestSession {
        cancel: test_cancel.clone(),
        auto_loaded_stt: false,
    });
    let (tx, _rx) = std::sync::mpsc::channel::<TtsPlaybackItem>();
    let pb_cancel = CancellationToken::new();
    *tts_playback_state().lock().await = Some(TtsPlaybackManager {
        tx,
        cancel: pb_cancel.clone(),
    });

    voice_shutdown().await;

    assert!(stt_state().lock().await.is_none(), "STT 会话已收");
    assert!(
        speaker_register_state().lock().unwrap().is_none(),
        "注册会话已收"
    );
    assert!(speaker_test_state().lock().await.is_none(), "测试会话已收");
    assert!(tts_playback_state().lock().await.is_none(), "播放会话已收");
    // 注入的令牌确实被 cancel（收口动了真格）
    assert!(reg_cancel.is_cancelled() && test_cancel.is_cancelled() && pb_cancel.is_cancelled());
    assert!(dialogue_state().lock().await.is_none());
}

// ── dialogue stop/reset 与 tts_playback stop 状态机 ──────────────────────

#[tokio::test]
async fn dialogue_stop_reset_and_playback_stop_state_machines() {
    let _guard = cov_lock();
    let h = VoiceHandler::new();

    // 空态：stop/reset 诚实报错
    *stt_state().lock().await = None;
    assert!(
        h.cmd_stt_dialogue_stop()
            .await
            .unwrap_err()
            .contains("not running")
    );
    assert!(
        h.cmd_stt_dialogue_reset()
            .await
            .unwrap_err()
            .contains("No dialogue session")
    );
    assert_eq!(
        h.cmd_tts_playback_stop().await.unwrap().unwrap()["was_running"],
        false
    );

    // 注入活动对话会话：stop 走完整收口（flush + relay 清注册 + 停播 + 清态）
    let (push, sink) = collecting_push();
    let out = make_dialogue_output(push);
    out.send_text("tail-words");
    let cancel = CancellationToken::new();
    *stt_state().lock().await = Some(SttSession {
        cancel: cancel.clone(),
        dialogue_output: Some(out.clone()),
    });
    *dialogue_state().lock().await = Some(out.clone());
    crate::voice_relay::set_relay_registration(Some(Arc::new(
        crate::voice_relay::RelayRegistration {
            session_key: "cov-sess".to_string(),
            voice_dir: std::path::PathBuf::from("."),
            config_dir: std::path::PathBuf::from("."),
            agent_loop: None,
        },
    )));
    let out_val = h.cmd_stt_dialogue_stop().await.unwrap().unwrap();
    assert_eq!(out_val["stopped"], true, "{out_val}");
    assert!(stt_state().lock().await.is_none());
    assert!(dialogue_state().lock().await.is_none());
    assert!(
        crate::voice_relay::relay_registration().is_none(),
        "relay 注册已清"
    );
    // flush 残句已推送
    assert!(
        sink.lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("tail-words")),
        "stop 时残句 flush 上屏: {:?}",
        sink.lock().unwrap()
    );

    // reset：注入态 → Ok；再注入后手动 reset 幂等
    let (push2, _sink2) = collecting_push();
    let out2 = make_dialogue_output(push2);
    *dialogue_state().lock().await = Some(out2.clone());
    assert_eq!(
        h.cmd_stt_dialogue_reset().await.unwrap().unwrap()["reset"],
        true
    );
    out2.send_text("buffered");
    h.cmd_stt_dialogue_reset().await.unwrap();
    assert!(out2.flush().is_none(), "reset 清空缓冲");
    *dialogue_state().lock().await = None;

    // tts_playback stop：注入活动播放会话 → stopped true
    let (tx, _rx) = std::sync::mpsc::channel::<TtsPlaybackItem>();
    let pb_cancel = CancellationToken::new();
    *tts_playback_state().lock().await = Some(TtsPlaybackManager {
        tx,
        cancel: pb_cancel.clone(),
    });
    let stop_val = h.cmd_tts_playback_stop().await.unwrap().unwrap();
    assert_eq!(stop_val["stopped"], true, "{stop_val}");
    assert!(tts_playback_state().lock().await.is_none());
    assert!(pb_cancel.is_cancelled());
}

// ── relay 入队早退与 barge-in ────────────────────────────────────────────

#[tokio::test]
async fn relay_enqueue_early_returns_and_barge_in_trigger() {
    let _guard = cov_lock();

    // 无注册：直接返回（不合成不播放）
    crate::voice_relay::set_relay_registration(None);
    relay_enqueue("hello world").await;

    // 有注册 + 空内容：分句为空 → 返回
    let tmp = tempfile::tempdir().unwrap();
    crate::voice_relay::set_relay_registration(Some(Arc::new(
        crate::voice_relay::RelayRegistration {
            session_key: "cov".to_string(),
            voice_dir: tmp.path().join("tools").join("voice"),
            config_dir: tmp.path().join("config"),
            agent_loop: None,
        },
    )));
    relay_enqueue("   ").await;
    crate::voice_relay::set_relay_registration(None);

    // barge-in 触发：无活动播放/播放会话/注册 → 纯前端标注路径。本函数
    // 设计运行于 spawn_blocking 音频线程（blocking_lock）——测试同样放
    // blocking 池，避免 runtime 线程内阻塞。
    let (push, sink) = collecting_push();
    let out = make_dialogue_output(push);
    let out2 = out.clone();
    tokio::task::spawn_blocking(move || trigger_barge_in(&*out2, None))
        .await
        .expect("barge-in 无活动现场不 panic");
    assert_eq!(
        sink.lock().unwrap().last().map(String::as_str),
        Some("barge_in:1")
    );
    // 带注册（agent_loop 缺席）→ cancel_session 臂跳过，标注照发
    let out3 = out.clone();
    let reg = crate::voice_relay::RelayRegistration {
        session_key: "cov2".to_string(),
        voice_dir: std::path::PathBuf::from("."),
        config_dir: std::path::PathBuf::from("."),
        agent_loop: None,
    };
    tokio::task::spawn_blocking(move || trigger_barge_in(&*out3, Some(&reg)))
        .await
        .expect("barge-in 带注册（无 agent loop）不 panic");
    assert_eq!(sink.lock().unwrap().len(), 2);
}

#[test]
fn barge_speaker_ok_gates() {
    let _guard = cov_lock();
    // 1) speaker 未启用：放行（引擎缺席也放行）
    *speaker_enabled_state().lock().unwrap() = false;
    assert!(barge_speaker_ok(&[0.1f32; 64], 16000));
    // 2) 启用 + 音频不足 0.5s：拒（宁可不打断）
    *speaker_enabled_state().lock().unwrap() = true;
    let short = vec![0.1f32; 4000]; // 0.25s @16k，低于 0.5s 门槛
    assert!(!barge_speaker_ok(&short, 16000));
    // 3) 启用 + 足量音频 + 引擎/管理器缺席：校验不可用 → 放行
    let enough = vec![0.1f32; 16000];
    assert!(barge_speaker_ok(&enough, 16000));
    *speaker_enabled_state().lock().unwrap() = false;
}

// ── speaker 注册冲突 / 阈值持久化 / status 投影 ──────────────────────────

#[tokio::test]
async fn speaker_register_conflict_threshold_and_status_projection() {
    let _guard = cov_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let h = VoiceHandler::new();

    // 注册占用中：start 二次进入诚实拒绝；空态 stop 诚实报错
    *speaker_register_state().lock().unwrap() = Some(SpeakerRegistration {
        name: "busy".to_string(),
        samples: std::sync::Mutex::new(Vec::new()),
        sample_rate: 16000,
        start_time: Instant::now(),
        cancel: CancellationToken::new(),
    });
    assert!(
        h.cmd_speaker_register_start(&cfg_dir, "owner")
            .unwrap_err()
            .contains("already in progress")
    );
    assert!(
        h.cmd_speaker_register_stop(&cfg_dir)
            .unwrap_err()
            .contains("录音时间过短"),
        "刚起步 = 时长不足"
    );
    assert!(
        speaker_register_state().lock().unwrap().is_none(),
        "stop 消费注入态（take 语义）"
    );

    // cancel：收注册 + 幂等成功
    let cancelled = h.cmd_speaker_register_cancel().unwrap().unwrap();
    assert_eq!(cancelled["cancelled"], true);
    assert!(speaker_register_state().lock().unwrap().is_none());
    assert_eq!(
        h.cmd_speaker_register_cancel().unwrap().unwrap()["cancelled"],
        true
    );

    // 阈值：越界拒绝；合法值落盘 + 热态同步
    assert!(h.cmd_speaker_set_threshold(&cfg_dir, 1.5).is_err());
    assert!(h.cmd_speaker_set_threshold(&cfg_dir, -0.1).is_err());
    let out = h
        .cmd_speaker_set_threshold(&cfg_dir, 0.42)
        .unwrap()
        .unwrap();
    assert!(
        (out["threshold"].as_f64().unwrap() - 0.42).abs() < 1e-6,
        "{out}"
    );
    let th = *speaker_threshold_state().lock().unwrap();
    assert!((th - 0.42).abs() < 1e-6, "{th}");
    let vp = read_voiceprint_config(&cfg_dir);
    assert!(
        (vp["threshold"].as_f64().unwrap() - 0.42).abs() < 1e-6,
        "阈值持久化: {vp}"
    );

    // 声纹表：写两个注册人 → list/status/remove 投影
    update_voiceprint_config(&cfg_dir, |v| {
        v["speakers"] = serde_json::json!({
            "alice": {"embedding": [1.0, 2.0]},
            "bob": {"embedding": [3.0]},
        });
        Ok(())
    })
    .unwrap();
    *speaker_enabled_state().lock().unwrap() = true;
    let st = h.cmd_speaker_status(&cfg_dir).await.unwrap().unwrap();
    assert_eq!(st["enabled"], true);
    assert!(
        (st["threshold"].as_f64().unwrap() - 0.42).abs() < 1e-6,
        "{st}"
    );
    let speakers: Vec<&str> = st["speakers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        speakers.contains(&"alice") && speakers.contains(&"bob"),
        "{st}"
    );
    assert_eq!(st["stt_dialogue_active"], false, "对话态空");
    let list = h.cmd_speaker_list(&cfg_dir).unwrap().unwrap();
    assert_eq!(list["speakers"].as_array().unwrap().len(), 2);

    // remove：manager 缺席跳过（仅声纹表 RMW），表内条目消失
    let rm = h.cmd_speaker_remove(&cfg_dir, "bob").unwrap().unwrap();
    assert_eq!(rm["removed"], true);
    let list2 = h.cmd_speaker_list(&cfg_dir).unwrap().unwrap();
    let names: Vec<&str> = list2["speakers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alice"]);
    *speaker_enabled_state().lock().unwrap() = false;
}

#[tokio::test]
async fn engine_status_dialogue_active_projection() {
    let _guard = cov_lock();
    let tmp = tempfile::tempdir().unwrap();
    let h = VoiceHandler::new();

    // 注入带对话输出的活动 STT 会话：dialogue-active 投影翻真
    let (push, _sink) = collecting_push();
    let out = make_dialogue_output(push);
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: Some(out),
    });
    let st = h.cmd_engine_status(&tmp.path()).await.unwrap().unwrap();
    assert_eq!(st["stt_ready"], false, "真引擎未装载");
    assert_eq!(st["tts_ready"], false);
    assert_eq!(st["speaker_ready"], false);
    assert_eq!(st["stt_dialogue_active"], true, "{st}");
    *stt_state().lock().await = None;
    let st2 = h.cmd_engine_status(&tmp.path()).await.unwrap().unwrap();
    assert_eq!(st2["stt_dialogue_active"], false);
}

// ── dispatch 错误面 ──────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_error_arms_offline() {
    let _guard = cov_lock();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&tmp);
    let h = VoiceHandler::new();
    let empty = serde_json::json!({});

    // setup / install_runtime 单飞闸：槽被占 = 拒
    *setup_cancel().lock().unwrap() = Some(CancellationToken::new());
    assert!(
        h.handle_cmd("setup", None, &ctx)
            .await
            .unwrap_err()
            .contains("已在进行中")
    );
    assert!(
        h.handle_cmd("install_runtime", None, &ctx)
            .await
            .unwrap_err()
            .contains("已在进行中")
    );
    // install_aec 无 setup_cancel 单飞闸（直入下载段）——occupied 断言不适用
    // stop_setup：有活动安装 = 取消成功
    let stopped = h
        .handle_cmd("stop_setup", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped["stopped"], true);
    *setup_cancel().lock().unwrap() = None;

    // install_model：缺 data / 缺 model 字段
    assert!(
        h.handle_cmd("install_model", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("install_model", Some(empty.clone()), &ctx)
            .await
            .unwrap_err()
            .contains("missing field: model")
    );
    // install_model：锁键占用（by-name 形态）
    install_locks()
        .lock()
        .unwrap()
        .insert("tts/kokoro".to_string());
    let err = h
        .handle_cmd(
            "install_model",
            Some(serde_json::json!({"model": "tts", "name": "kokoro"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("正在安装中"), "{err}");
    assert!(
        !install_locks().lock().unwrap().contains("tts/kokoro") == false,
        "占用臂不摘锁"
    );
    install_locks().lock().unwrap().remove("tts/kokoro");
    // install_model：config.toml 缺失 = spawn_blocking 内诚实 Err（无网络）
    let err = h
        .handle_cmd(
            "install_model",
            Some(serde_json::json!({"model": "tts"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("config.toml not found") || err.contains("install failed"),
        "{err}"
    );
    assert!(setup_cancel().lock().unwrap().is_none(), "失败后闸清空");

    // speaker 命令 NUL 名闸
    assert!(
        h.handle_cmd(
            "speaker_register_start",
            Some(serde_json::json!({"name": "a\0b"})),
            &ctx
        )
        .await
        .unwrap_err()
        .contains("NUL")
    );
    assert!(
        h.handle_cmd(
            "speaker_remove",
            Some(serde_json::json!({"name": "a\0b"})),
            &ctx
        )
        .await
        .unwrap_err()
        .contains("NUL")
    );
    assert!(
        h.handle_cmd("speaker_remove", Some(empty.clone()), &ctx)
            .await
            .unwrap_err()
            .contains("missing field: name")
    );
    assert!(
        h.handle_cmd("speaker_set_threshold", Some(empty.clone()), &ctx)
            .await
            .unwrap_err()
            .contains("missing field: threshold")
    );

    // 其余缺 data/缺字段臂
    assert!(
        h.handle_cmd("set_model", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("config_set", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("voice_config_set", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("chat_config_set", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("tts", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("engine_start", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("engine_stop", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("pipeline_start", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("pipeline_stop", None, &ctx)
            .await
            .unwrap_err()
            .contains("missing data")
    );
    assert!(
        h.handle_cmd("engine_start", Some(empty.clone()), &ctx)
            .await
            .unwrap_err()
            .contains("missing field: model")
    );
    assert!(
        h.handle_cmd("engine_stop", Some(empty.clone()), &ctx)
            .await
            .unwrap_err()
            .contains("missing field: model")
    );
    // 未知命令
    assert!(
        h.handle_cmd("bogus_cmd", None, &ctx)
            .await
            .unwrap_err()
            .contains("unknown command")
    );
}

// ── init_engines 全关 no-op + 纯 FS helper 矩阵 ──────────────────────────

#[tokio::test]
async fn init_engines_all_disabled_is_noop() {
    let _guard = cov_lock();
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join(VOICE_CONFIG_FILENAME),
        serde_json::json!({
            "stt_enabled": false,
            "tts_enabled": false,
            "speaker_enabled": false,
        })
        .to_string(),
    )
    .unwrap();
    // 三开关全关：不触任何引擎初始化（即时返回）
    init_engines_from_config(tmp.path()).await;
    assert!(stt_engine_state().lock().unwrap().is_none());
    assert!(tts_engine_state().lock().unwrap().is_none());
    assert!(speaker_engine_state().lock().unwrap().is_none());
}

#[test]
fn pure_helper_matrix() {
    // model_label：类别 → 人读标签；未知键原样透传
    assert_eq!(model_label("tts"), "TTS");
    assert_eq!(model_label("stt_stream"), "流式STT");
    assert_eq!(model_label("speaker"), "声纹");
    assert_eq!(model_label("unknown-thing"), "unknown-thing");

    // punctuate_if_loaded：引擎缺席 = 原文透传
    assert_eq!(punctuate_if_loaded("raw text"), "raw text");

    // has_onnx_file / check_model_subdir_any
    let tmp = tempfile::tempdir().unwrap();
    let models = tmp.path().join("models");
    assert!(!has_onnx_file(&models), "目录缺席 = false");
    assert!(!check_model_subdir_any(&models), "目录缺席 = false");
    let variant = models.join("tts").join("v1");
    std::fs::create_dir_all(&variant).unwrap();
    assert!(
        !check_model_subdir_any(&models.join("tts")),
        "变体目录无 onnx = false"
    );
    std::fs::write(variant.join("model.onnx"), b"x").unwrap();
    assert!(has_onnx_file(&variant));
    assert!(
        check_model_subdir_any(&models.join("tts")),
        "变体含 onnx = true"
    );
    // helper 只查给定目录下的直接 .onnx 文件（不递归）——变体层目录不算
    assert!(
        !has_onnx_file(&models.join("tts")),
        "tts 层只有目录没有直接 onnx 文件"
    );

    // installed_variants：显式排序
    std::fs::create_dir_all(models.join("tts").join("v2")).unwrap();
    std::fs::create_dir_all(models.join("tts").join("v10")).unwrap();
    std::fs::write(models.join("tts").join("loose.onnx"), b"x").unwrap(); // 文件不算变体
    assert_eq!(
        installed_variants(&models, "tts"),
        vec!["v1".to_string(), "v10".to_string(), "v2".to_string()],
        "字典序 + 只收目录"
    );
    assert!(
        installed_variants(&models, "ghost").is_empty(),
        "类别缺席 = 空"
    );

    // current_model_name_of：六类投影 + 未知类别
    let cfg = nemesis_voice::AppConfig::load_or_default(&tmp.path().join("nope.toml"));
    for cat in ["stt", "stt_stream", "vad", "tts", "punct", "speaker"] {
        assert!(current_model_name_of(&cfg, cat).is_some(), "{cat} 有默认名");
    }
    assert_eq!(current_model_name_of(&cfg, "boom"), None);
}
