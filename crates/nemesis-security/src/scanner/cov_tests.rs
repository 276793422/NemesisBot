// scanner.rs 覆盖率补充测试（ScanChain file/content/directory 三扫描全臂
// + scan_tool_invocation 各工具臂 + ClamAVEngine detect/setup/download 全臂
// ——download 用本地一次性 HTTP 服务 + 内存构造 zip，不触外网）。
//
// 豁免：
// - 591：target_executables 非 Windows 臂（cfg! 恒假）。
// - 691-694 / 739：下载写盘失败 / flush 失败臂——需已打开句柄失效或磁盘
//   故障，单测不可确定性构造。
// - 1037：clamd「并发扫描刚把它重启」臂——需真实 clamd 守护进程生命周期
//   （先 ping 失败再 ping 成功），单测环境不可得。

use super::*;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
// 注意：本文件同时用了 tokio::io::AsyncWriteExt 与 std::io::Write——
// ZipWriter 的 write_all 走完全限定调用（见 zip_bytes），不引入歧义 import。

// ---------------------------------------------------------------------------
// 可配置 mock 引擎
// ---------------------------------------------------------------------------

struct CovEngine {
    name: &'static str,
    ready: bool,
    file_infected: bool,
    content_infected: bool,
    dir_infected: bool,
}

impl CovEngine {
    fn clean(name: &'static str) -> Box<Self> {
        Box::new(Self {
            name,
            ready: true,
            file_infected: false,
            content_infected: false,
            dir_infected: false,
        })
    }
    fn infected(name: &'static str) -> Box<Self> {
        Box::new(Self {
            name,
            ready: true,
            file_infected: true,
            content_infected: true,
            dir_infected: true,
        })
    }
    fn not_ready(name: &'static str) -> Box<Self> {
        Box::new(Self {
            name,
            ready: false,
            file_infected: true,
            content_infected: true,
            dir_infected: true,
        })
    }
}

#[async_trait]
impl VirusScanner for CovEngine {
    fn name(&self) -> &str {
        self.name
    }
    async fn get_info(&self) -> EngineInfo {
        EngineInfo {
            name: self.name.to_string(),
            version: String::new(),
            address: String::new(),
            ready: self.ready,
            start_time: String::new(),
        }
    }
    async fn start(&self) -> Result<(), String> {
        Ok(())
    }
    async fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    async fn is_ready(&self) -> bool {
        self.ready
    }
    async fn scan_file(&self, path: &Path) -> ScanResult {
        if self.file_infected {
            ScanResult::with_threats(self.name, "Cov.Test", &path.to_string_lossy())
        } else {
            ScanResult::clean_with_path(self.name, &path.to_string_lossy())
        }
    }
    async fn scan_content(&self, _content: &[u8]) -> ScanResult {
        if self.content_infected {
            ScanResult::with_threats(self.name, "Cov.Content", "")
        } else {
            ScanResult::clean_from(self.name)
        }
    }
    async fn scan_directory(&self, _dir: &Path) -> Vec<ScanResult> {
        if self.dir_infected {
            vec![ScanResult::with_threats(self.name, "Cov.Dir", "bad.exe")]
        } else {
            Vec::new()
        }
    }
    async fn get_database_status(&self) -> DatabaseStatus {
        DatabaseStatus::default()
    }
    async fn update_database(&self) -> Result<(), String> {
        Ok(())
    }
    fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        HashMap::new()
    }
}

// ---------------------------------------------------------------------------
// ScanChain 扫描路径
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scan_chain_file_paths_all_branches() {
    // 空链 → clean。
    let empty = ScanChain::with_defaults();
    assert!(empty.scan_file(Path::new("x.txt")).await.clean);

    // 未就绪引擎被跳过（即便会报感染）。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::not_ready("nr"));
    let r = chain.scan_file(Path::new("x.txt")).await;
    assert!(r.clean && r.results.is_empty(), "未就绪必须跳过");

    // 干净引擎跑完整个循环（for 收尾 brace 区）。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::clean("e1"));
    let r = chain.scan_file(Path::new("x.txt")).await;
    assert!(r.clean && !r.blocked && r.results.len() == 1);

    // 短路：e1 干净 + e2 感染 → blocked，结果含两家。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::clean("e1"));
    chain.add_engine(CovEngine::infected("e2"));
    let r = chain.scan_file(Path::new("x.txt")).await;
    assert!(!r.clean && r.blocked);
    assert_eq!(r.engine, "e2");
    assert_eq!(r.virus, "Cov.Test");
    assert_eq!(r.results.len(), 2);
}

#[tokio::test]
async fn scan_chain_content_and_directory_paths() {
    let empty = ScanChain::with_defaults();
    assert!(empty.scan_content(b"data").await.clean);
    assert!(empty.scan_directory(Path::new(".")).await.clean);

    // 内容扫描：未就绪跳过 → 干净 → 感染短路（path 为空串形态）。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::not_ready("nr"));
    assert!(chain.scan_content(b"data").await.clean);

    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::clean("e1"));
    assert!(chain.scan_content(b"data").await.clean);

    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::clean("e1"));
    chain.add_engine(CovEngine::infected("e2"));
    let r = chain.scan_content(b"EICAR").await;
    assert!(!r.clean && r.blocked && r.engine == "e2");
    assert_eq!(r.path, "");

    // 目录扫描：感染结果短路，path 取自结果本体。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::infected("e1"));
    let r = chain.scan_directory(Path::new("somedir")).await;
    assert!(!r.clean && r.blocked && r.engine == "e1");
    assert_eq!(r.path, "bad.exe");
}

#[tokio::test]
async fn scan_tool_invocation_all_tool_arms() {
    // 禁用链 → 放行（即便有引擎）。
    let mut chain = ScanChain::with_defaults();
    chain.add_engine(CovEngine::infected("e1"));
    let (allowed, err) = chain
        .scan_tool_invocation("write_file", &serde_json::json!({"content": "x"}))
        .await;
    assert!(allowed && err.is_none(), "禁用链必须放行");

    // 启用但无引擎 → 放行（1670）。
    let chain = ScanChain::with_defaults();
    chain.set_enabled(true);
    let (allowed, err) = chain
        .scan_tool_invocation("write_file", &serde_json::json!({"content": "x"}))
        .await;
    assert!(allowed && err.is_none());

    // 启用 + 感染引擎：write_file 内容臂。
    let mut chain = ScanChain::with_defaults();
    chain.set_enabled(true);
    chain.add_engine(CovEngine::infected("e1"));
    let (allowed, err) = chain
        .scan_tool_invocation(
            "write_file",
            &serde_json::json!({"path": "a.txt", "content": "payload"}),
        )
        .await;
    assert!(!allowed);
    assert!(err.unwrap().contains("e1"));

    // write_file 干净 → 放行；空 content → 放行。
    let mut chain = ScanChain::with_defaults();
    chain.set_enabled(true);
    chain.add_engine(CovEngine::clean("e1"));
    let (allowed, err) = chain
        .scan_tool_invocation(
            "write_file",
            &serde_json::json!({"path": "a.txt", "content": "ok"}),
        )
        .await;
    assert!(allowed && err.is_none());
    let (allowed, err) = chain
        .scan_tool_invocation(
            "write_file",
            &serde_json::json!({"path": "a.txt", "content": ""}),
        )
        .await;
    assert!(allowed && err.is_none());

    // download / exec / screen_capture / install_skill：文件扫描臂（感染）。
    for tool in ["download", "exec", "screen_capture", "install_skill"] {
        let mut chain = ScanChain::with_defaults();
        chain.set_enabled(true);
        chain.add_engine(CovEngine::infected("e1"));
        let args = serde_json::json!({"path": "evil.bin", "save_path": "evil.bin"});
        let (allowed, err) = chain.scan_tool_invocation(tool, &args).await;
        assert!(!allowed, "{tool} 必须拦");
        assert!(err.unwrap().contains("evil.bin"));
    }

    // web_fetch：content 内联字段臂。
    let mut chain = ScanChain::with_defaults();
    chain.set_enabled(true);
    chain.add_engine(CovEngine::infected("e1"));
    let (allowed, err) = chain
        .scan_tool_invocation(
            "web_fetch",
            &serde_json::json!({"content": "<html>bad</html>"}),
        )
        .await;
    assert!(!allowed);
    assert!(err.unwrap().contains("web_fetch"));

    // cron：command 内容臂。
    let mut chain = ScanChain::with_defaults();
    chain.set_enabled(true);
    chain.add_engine(CovEngine::infected("e1"));
    let (allowed, err) = chain
        .scan_tool_invocation("cron", &serde_json::json!({"command": "curl evil"}))
        .await;
    assert!(!allowed);
    assert!(err.unwrap().contains("cron"));

    // 未知工具名 → 落默认臂放行。
    let mut chain = ScanChain::with_defaults();
    chain.set_enabled(true);
    chain.add_engine(CovEngine::infected("e1"));
    let (allowed, err) = chain
        .scan_tool_invocation("totally_unknown", &serde_json::json!({"path": "x"}))
        .await;
    assert!(allowed && err.is_none());
}

#[test]
fn extract_paths_from_args_tool_variants() {
    let chain = ScanChain::with_defaults();

    let paths = chain.extract_paths_from_args(
        "write_file",
        &serde_json::json!({"path": "a.txt", "file_path": "b.txt"}),
    );
    assert_eq!(paths, vec!["a.txt", "b.txt"]);

    let paths = chain.extract_paths_from_args(
        "download",
        &serde_json::json!({"save_path": "s.bin", "path": "p.bin"}),
    );
    assert_eq!(paths, vec!["s.bin", "p.bin"]);

    let paths = chain.extract_paths_from_args(
        "exec",
        &serde_json::json!({"command": "run ./x/y.exe and z.dll"}),
    );
    assert_eq!(paths, vec!["./x/y.exe", "z.dll"]);

    let paths = chain.extract_paths_from_args(
        "screen_capture",
        &serde_json::json!({"save_path": "shot.png"}),
    );
    assert_eq!(paths, vec!["shot.png"]);

    let paths =
        chain.extract_paths_from_args("install_skill", &serde_json::json!({"path": "skill/"}));
    assert_eq!(paths, vec!["skill/"]);

    assert!(
        chain
            .extract_paths_from_args("mystery", &serde_json::json!({"path": "x"}))
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// ClamAVEngine：detect_install_path / setup / download
// ---------------------------------------------------------------------------

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-scan-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn detect_install_path_found_and_missing() {
    let engine = ClamAVEngine::new(ClamAVEngineConfig::default());
    let dir = temp_dir("detect");

    // 空目录 → 未找到。
    let err = engine.detect_install_path(&dir).unwrap_err();
    assert!(err.contains("target executable not found"), "{err}");

    // 嵌套 clamd.exe → 返回其所在目录。
    let nested = dir.join("release").join("bin");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("clamd.exe"), b"fake").unwrap();
    let found = engine.detect_install_path(&dir).unwrap();
    assert!(found.ends_with("bin"), "{found}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn setup_null_and_valid_and_invalid_config() {
    let engine = ClamAVEngine::new(ClamAVEngineConfig::default());

    // null 配置 = no-op。
    engine.setup(&serde_json::Value::Null).unwrap();

    // 合法配置覆写。
    engine
        .setup(&serde_json::json!({"url": "http://cov.example/x.zip", "address": "127.0.0.1:3310"}))
        .unwrap();
    assert_eq!(engine.get_clamav_path(), "");

    // 非法类型 → Err。
    engine.setup(&serde_json::json!(42)).unwrap_err();
}

#[tokio::test]
async fn download_rejects_empty_url_and_uncreatable_dir() {
    let engine = ClamAVEngine::new(ClamAVEngineConfig::default());
    let dir = temp_dir("nourl");

    // 空 URL → 直接 Err（不触网）。
    let err = engine
        .download(
            dir.to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("no download URL"), "{err}");

    // URL 有值但目标目录不可创建（父位是文件）→ create_dir_all 臂（648），
    // 同样不触网。
    std::fs::write(dir.join("blocker"), b"file").unwrap();
    let bad_dir = dir.join("blocker").join("sub");
    let engine2 = ClamAVEngine::new(ClamAVEngineConfig {
        url: "http://127.0.0.1:1/x.zip".into(),
        ..Default::default()
    });
    let err = engine2
        .download(
            bad_dir.to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("failed to create directory"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 本地一次性 HTTP 服务（download 网络臂）
// ---------------------------------------------------------------------------

enum Resp {
    Bytes(Vec<u8>),
    /// 只收请求不回包（供取消臂用）。
    Stall,
}

async fn spawn_http_server(script: Vec<Resp>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        for resp in script {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            // 读到请求头结束。
            let mut buf = [0u8; 4096];
            let mut total = 0;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let n = sock.read(&mut buf[total..]).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            })
            .await;
            match resp {
                Resp::Bytes(b) => {
                    let _ = sock.write_all(&b).await;
                    let _ = sock.shutdown().await;
                }
                Resp::Stall => {
                    // 先回响应头（reqwest::get 在头阶段不感知取消令牌），
                    // 再扣住连接不回体 → 客户端流挂起，取消臂才可打断。
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
                        .await;
                    let _ = sock.flush().await;
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    return;
                }
            }
        }
    });
    port
}

fn http_200(body: &[u8]) -> Vec<u8> {
    let mut resp =
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    resp.extend_from_slice(body);
    resp
}

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut w = zip::ZipWriter::new(cursor);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        std::io::Write::write_all(&mut w, data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[tokio::test]
async fn download_success_extracts_and_detects_install_path() {
    let body = zip_bytes(&[("clamav/clamd.exe", b"fake clamd exe")]);
    let port = spawn_http_server(vec![Resp::Bytes(http_200(&body))]).await;

    let engine = ClamAVEngine::new(ClamAVEngineConfig {
        url: format!("http://127.0.0.1:{port}/clamav.zip"),
        ..Default::default()
    });
    let dir = temp_dir("dl-ok");

    let progress: Arc<dyn Fn(u64, u64) + Send + Sync> = Arc::new(|done: u64, total: u64| {
        assert!(done >= total, "进度回调参数必须一致");
    });
    engine
        .download(
            dir.to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            Some(progress),
        )
        .await
        .unwrap();

    assert!(
        dir.join("clamav").join("clamd.exe").exists(),
        "zip 必须解出"
    );
    assert!(
        engine.get_clamav_path().ends_with("clamav"),
        "下载后自动探测安装路径: {}",
        engine.get_clamav_path()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_rejects_non_success_status() {
    let port = spawn_http_server(vec![Resp::Bytes(
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
    )])
    .await;

    let engine = ClamAVEngine::new(ClamAVEngineConfig {
        url: format!("http://127.0.0.1:{port}/clamav.zip"),
        ..Default::default()
    });
    let dir = temp_dir("dl-500");
    let err = engine
        .download(
            dir.to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("status"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_fails_when_temp_file_uncreatable() {
    let body = zip_bytes(&[("clamav/clamd.exe", b"x")]);
    let port = spawn_http_server(vec![Resp::Bytes(http_200(&body))]).await;

    let engine = ClamAVEngine::new(ClamAVEngineConfig {
        url: format!("http://127.0.0.1:{port}/clamav.zip"),
        ..Default::default()
    });
    let dir = temp_dir("dl-tmp");
    // 固定临时名 clamav-download.zip 被目录占位 → File::create 必败（669）。
    std::fs::create_dir_all(dir.join("clamav-download.zip")).unwrap();

    let err = engine
        .download(
            dir.to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("failed to create temp file"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn download_cancellation_aborts_mid_stream() {
    // 只回头不回体 → 客户端流挂起 → 取消令牌打断（cancel 臂）。
    let port = spawn_http_server(vec![Resp::Stall]).await;

    let engine = ClamAVEngine::new(ClamAVEngineConfig {
        url: format!("http://127.0.0.1:{port}/clamav.zip"),
        ..Default::default()
    });
    let dir = temp_dir("dl-cancel");
    let token = tokio_util::sync::CancellationToken::new();

    let dl = tokio::spawn({
        let dir = dir.clone();
        let token = token.clone();
        async move { engine.download(dir.to_str().unwrap(), token, None).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    token.cancel();
    let err = tokio::time::timeout(std::time::Duration::from_secs(5), dl)
        .await
        .expect("取消后必须及时返回")
        .unwrap()
        .unwrap_err();
    assert!(err.contains("cancel"), "{err}");

    let _ = std::fs::remove_dir_all(&dir);
}
