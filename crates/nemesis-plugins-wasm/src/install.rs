//! 装配漏斗（install）：源目录 → 九步准入 → 注册 + lockfile。
//!
//! 步骤（缺一不可，顺序固定）：
//! ① manifest 结构校验（含签名剥离后解析）→ ② 验签四态（Blocked 拒绝）
//! → ③ wasm sha256 重算比对 + 64MiB 上限 → ④ 病毒扫描（ScanChain；无引擎
//! 时跳过并注记——第 7 层缺席不降低其余闸）→ ⑤ 审批卡（InstallApprover，
//! 启动装载跳过——安装期已裁决）→ ⑥ 编译 + get-metadata 对账（observer 跳
//! 过对账）→ ⑦ lockfile → ⑧ 落位 plugins/<slug>/ + 数据目录 → ⑨ 注册。
//!
//! lockfile 在落位之前写（lockfile-first，与 uninstall 的 remove-lockfile
//! 先行对称，2026-09-30 复查 #5）：lockfile 失败 = 零突变干净中止（优于
//! 「报失败但新载荷已生效」的谎报）；落位/注册失败 = lockfile 元数据超前
//! （诚实失败，重装自愈——load_one 的 sha 自洽比对不读 lockfile，不破装载）。
//!
//! `allow_unsigned` 只豁免「无签名」（ReviewRequired），不豁免「签名无效」。

use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::error::PluginError;
use crate::host_impl::SecretResolver;
use crate::manifest::{PluginKind, PluginManifest, valid_slug};
use crate::registry::{
    PluginManager, PolicyMetaSnapshot, RegisteredPlugin, StepMetaSnapshot, ToolMetaSnapshot,
};
use crate::trust::{PluginTrustState, VerificationOutcome, trust_state_for, verify_manifest};

/// wasm 载荷字节上限（64MiB；wasm 二进制正常在几百 KB~几 MB）。
pub const WASM_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// 同进程装配漏斗互斥（Dashboard 双击 / WSAPI 并发 install/uninstall/
/// enable/disable 同 slug 时，载荷目录重建、lockfile/实例配置读-改-写与
/// 内存启用位翻转会竞争出错配/丢条目/「写盘失败但内存已翻转」假成功
/// —— 交付审查 3.3/ + 复查 #7：set_enabled_plugin
/// 也走此闸，先写盘成功再翻内存位）。跨进程 CLI vs gateway 不在此闸内，
/// 后果：不同 slug 并发 → lockfile 读-改-写丢条目（CLI 管理面不自愈）；
/// 同 slug 并发 → 载荷与 lockfile 混版本（load_one 的 sha 自洽比对拒载，
/// 诚实失败不装错代码）。
pub(crate) static FUNNEL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// lockfile 形态（`plugins/lockfile.json`）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PluginLockfile {
    /// 形态代际。
    pub version: u32,
    /// slug → 安装快照。
    #[serde(default)]
    pub plugins: std::collections::BTreeMap<String, LockfileEntry>,
}

/// lockfile 单条目。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LockfileEntry {
    /// manifest 版本。
    pub version: String,
    /// wasm sha256（hex64；纯声明式插件（无载荷）= 空串——无字节可比对）。
    pub wasm_sha256: String,
    /// 信任四态字符串。
    pub trust: String,
    /// 安装时间（Unix 毫秒）。
    pub installed_at_ms: u64,
    /// 签名者公钥前 16 hex（未签名 = 空）。
    pub signed_by: String,
    /// 装配自动注册成功的 connector id（域侧批：审计面 + 卸载
    /// 提示；只记事实——冲突不覆盖/IO 失败的不在此列。`#[serde(default)]`
    /// 旧形态 lockfile 零迁移）。
    #[serde(default)]
    pub connectors: Vec<String>,
}

/// 审批卡内容（装配漏斗第⑤步入参；Dashboard/CLI 渲染面）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallReview {
    /// 插件 slug。
    pub slug: String,
    /// 人读名称。
    pub name: String,
    /// 版本。
    pub version: String,
    /// kind（tool/observer/policy/panel）。
    pub kind: String,
    /// 信任四态。
    pub trust_state: String,
    /// 信任级别（verified/community/无签名=空）。
    pub trust_level: String,
    /// 签名者公钥前 16 hex。
    pub signer_key: String,
    /// 出站 allowlist（空 = 无出站）。
    pub egress: Vec<String>,
    /// 凭据声明名。
    pub x_secret: Vec<String>,
    /// wasm 字节数。
    pub wasm_bytes: u64,
    /// wasm sha256（hex64，第③步重算值——审批卡上的人比对照锚点，与
    /// manifest/lockfile 记账同源。 复查 #12：装的是哪个载荷，
    /// 审批者有权知道。纯声明式 = 空串）。
    pub wasm_sha256: String,
    /// 面板页面数（kind=panel 的声明面摘要；其他 kind = 0）。
    pub ui_pages: usize,
    /// flow 剧本模板数（kind=panel 的声明面摘要；其他 kind = 0）。
    pub flow_templates: usize,
    /// 包内 connector.toml 声明的 connector id（域侧批：装前
    /// 可见「将注册 connector: gitea」；解析失败到不了审批卡——①段已拒）。
    pub connectors: Vec<String>,
    /// manifest limits 被忽略的放宽请求。
    pub limits_ignored: Vec<String>,
    /// 来源目录。
    pub source_dir: String,
}

/// 安装审批面（gateway=审批卡异步等答；CLI=--yes 自动同意）。
pub trait InstallApprover: Send + Sync {
    /// 批准与否（Err = 审批通道本身故障，视为拒绝并携带原因）。
    fn approve<'a>(
        &'a self,
        review: &'a InstallReview,
    ) -> futures::future::BoxFuture<'a, Result<bool, String>>;

    /// 是否具备「人批卡豁免无签名」能力（开关补漏斗收口）：
    /// true = gateway 审批卡面（无签名载荷转卡裁决，批准 = 人显式接受，
    /// 与 `install --allow-unsigned` 同一留档语义）；false（缺省）= CLI
    /// `--yes`/headless——无签名豁免维持显式旗标纪律，开关路径从紧拒绝。
    fn can_approve_unsigned(&self) -> bool {
        false
    }
}

/// 装配漏斗 ①-⑧ 的产出（已批准、已编译、已落位；未注册——装载由
/// 开关驱动，见 [`PluginInstaller::set_enabled`]）。
pub struct ApprovedInstall {
    /// 已校验 manifest。
    pub manifest: PluginManifest,
    /// 信任四态。
    pub trust_state: PluginTrustState,
    /// 已编译组件句柄（壳侧）。
    pub component: u64,
    /// 工具面快照（kind=tool 时非空）。
    pub tool_meta: Option<ToolMetaSnapshot>,
    /// 策略面快照（kind=policy 时非空）。
    pub policy_meta: Option<PolicyMetaSnapshot>,
    /// flow 步骤面快照（kind=flow-step 时非空； guest step 批）。
    pub step_meta: Option<StepMetaSnapshot>,
    /// 装配自动注册**成功**的 connector id（⑥.5 事实面；冲突不覆盖/IO
    /// 失败者不在列，与 lockfile 记账同源）。
    pub connectors: Vec<String>,
}

/// `install` 的返回（第⑨步结果：装载与否由开关决定，调用方按此展示）。
#[derive(Clone, Debug)]
pub struct InstallOutcome {
    /// 插件 slug。
    pub slug: String,
    /// manifest 版本。
    pub version: String,
    /// 信任四态。
    pub trust: PluginTrustState,
    /// 非空 = 本次已装载注册（升级且开关原本开着）。
    pub registered: Option<Arc<crate::registry::RegisteredPlugin>>,
}

/// 始终同意（headless / 测试 / 显式 `--yes`）。
pub struct AutoApprove;

impl InstallApprover for AutoApprove {
    fn approve<'a>(
        &'a self,
        _review: &'a InstallReview,
    ) -> futures::future::BoxFuture<'a, Result<bool, String>> {
        Box::pin(async { Ok(true) })
    }
}

/// 装配器（漏斗执行体；持注册表与验签/扫描面）。
pub struct PluginInstaller {
    /// 注册表。
    pub manager: Arc<PluginManager>,
    /// 签名验证器（plugin 专用 TrustStore 文件 `<workspace>/config/plugin_trust.json`）。
    pub verifier: nemesis_security::signature::SignatureVerifier,
    /// 病毒扫描链（None = 无引擎，第④步诚实跳过）。
    pub scanner: Option<Arc<tokio::sync::RwLock<nemesis_security::scanner::ScanChain>>>,
    /// 审批面。
    pub approver: Arc<dyn InstallApprover>,
}

impl PluginInstaller {
    /// 构建装配器（TrustStore 指向 workspace config/plugin_trust.json）。
    pub fn new(
        manager: Arc<PluginManager>,
        approver: Arc<dyn InstallApprover>,
        scanner: Option<Arc<tokio::sync::RwLock<nemesis_security::scanner::ScanChain>>>,
    ) -> Self {
        let trust_path = manager
            .workspace_root()
            .join("config")
            .join("plugin_trust.json");
        Self {
            manager,
            verifier: nemesis_security::signature::SignatureVerifier::with_persistence(trust_path),
            scanner,
            approver,
        }
    }

    /// 装配漏斗（CLI / WSAPI install 共用）。新纪律：安装≠
    /// 加载——装配成功只落载荷与 lockfile，开关记录写入
    /// `config.plugin.wasm.json`（新装一律 `enabled=false` 不注册；升级保留
    /// 用户既有开关位，开着 → 摘旧重注册=热升级）。装载走显式开关
    ///（Dashboard 插件页 / `plugin enable`）。
    pub async fn install(
        &self,
        source_dir: &Path,
        allow_unsigned: bool,
    ) -> Result<InstallOutcome, PluginError> {
        let _funnel = FUNNEL_LOCK.lock().await;
        let source_dir = source_dir
            .canonicalize()
            .map_err(|e| PluginError::Manifest(format!("source dir: {e}")))?;
        let approved = self.install_core(&source_dir, allow_unsigned).await?;
        self.record_switch_and_load(approved).await
    }

    /// 第⑨步：开关记录 + 按需装载（`install` 收尾段；enable 补跑漏斗的
    /// 手工拷入路径不走这里——set_enabled 自己写开关，见 [`Self::set_enabled`]）。
    async fn record_switch_and_load(
        &self,
        approved: ApprovedInstall,
    ) -> Result<InstallOutcome, PluginError> {
        let config_dir = self.manager.workspace_root().join("config");
        // 读路径触发旧 per-slug 实例文件惰性迁移（enabled 强制 false）。
        let prior = nemesis_config::plugin_wasm::load_switches(&config_dir)
            .remove(approved.manifest.slug.as_str());
        let slug = approved.manifest.slug.clone();
        let version = approved.manifest.version.clone();
        let registered = match prior {
            None => {
                // 新装：记录缺省关；不注册。
                nemesis_config::plugin_wasm::set_enabled(&config_dir, &slug, false)
                    .map_err(PluginError::Io)?;
                tracing::info!(
                    slug = %slug,
                    "[WasmPlugin] 安装完成（默认不加载——Dashboard 插件页开加载开关或 plugin enable）"
                );
                None
            }
            Some(sw) if sw.enabled => {
                // 升级且用户此前开着：摘旧注册（防 policy 接管点自撞）再重装
                // = 热升级；register 失败 = 插件从注册表摘下但开关仍开（诚实
                // 可见态，重启装载再试）。
                self.manager.unregister(&slug);
                let reg = self
                    .manager
                    .register(
                        approved.manifest,
                        approved.trust_state,
                        approved.component,
                        approved.tool_meta,
                        approved.policy_meta,
                        approved.step_meta,
                    )
                    .map_err(|e| {
                        tracing::warn!(
                            slug = %slug,
                            error = %e,
                            "[WasmPlugin] 热升级重注册失败（开关保持开启，重启后重试）"
                        );
                        e
                    })?;
                Some(reg)
            }
            Some(_) => {
                // 升级但用户此前关着：只换盘上载荷，不装载。
                tracing::info!(
                    slug = %slug,
                    "[WasmPlugin] 升级完成（开关关闭，载荷已更新；打开开关后生效）"
                );
                None
            }
        };
        Ok(InstallOutcome {
            slug,
            version,
            trust: approved.trust_state,
            registered,
        })
    }

    /// 装配漏斗 ①-⑧（无第⑨步；`install` 与 `set_enabled` 的手工拷入路径
    /// 共用）。
    async fn install_core(
        &self,
        source_dir: &Path,
        allow_unsigned: bool,
    ) -> Result<ApprovedInstall, PluginError> {
        // ① manifest 结构校验 + ② 验签四态
        let raw = std::fs::read_to_string(source_dir.join("plugin.toml"))
            .map_err(|e| PluginError::Manifest(format!("read plugin.toml: {e}")))?;
        let (manifest, outcome) = verify_manifest(&raw, &self.verifier)?;
        let trust_state = trust_state_for(&outcome, allow_unsigned)?;

        // ①.5 包内 connector.toml 预检（域侧批）：解析失败 =
        // 拒绝安装（fail-closed，与 sha mismatch 同级——不装「看起来装好
        // 但 connector 不可用」的插件；此处零突变，早拒早干净）。
        let bundled_connector = self.parse_bundled_connector(source_dir)?;

        // 纯声明式面板（无载荷）：③④⑥ 三步诚实跳过——无字节可比对/
        // 可扫描/可编译，扩展点本体（ui/flow 段）全由宿主引擎兑现；
        // manifest 校验已保证声明式必为 panel 且至少一个页面。
        let declarative = manifest.wasm_sha256.is_none();

        // ③ wasm sha256 重算 + 大小上限（上限在读盘前用 metadata 判，
        // 64MiB 读进内存的 OOM 面提前拦）
        let mut wasm_len = 0u64;
        let mut actual_hash = String::new();
        let mut wasm_bytes: Vec<u8> = Vec::new();
        let mut scan_note: Option<String> = None;
        if !declarative {
            ensure_plain_filename(&manifest.wasm)?;
            let wasm_path = source_dir.join(&manifest.wasm);
            wasm_len = std::fs::metadata(&wasm_path)
                .map_err(|e| PluginError::Wasm(format!("stat {}: {e}", wasm_path.display())))?
                .len();
            if wasm_len > WASM_MAX_BYTES {
                return Err(PluginError::Wasm(format!(
                    "wasm exceeds {WASM_MAX_BYTES} bytes ({wasm_len})"
                )));
            }
            wasm_bytes = std::fs::read(&wasm_path)
                .map_err(|e| PluginError::Wasm(format!("read {}: {e}", wasm_path.display())))?;
            actual_hash = hex::encode(Sha256::digest(&wasm_bytes));
            let expect = manifest.wasm_sha256.as_deref().unwrap_or_default();
            if actual_hash != expect.to_lowercase() {
                return Err(PluginError::Wasm(format!(
                    "wasm sha256 mismatch: manifest {expect} actual {actual_hash}"
                )));
            }

            // ④ 病毒扫描（无引擎跳过并注记）
            scan_note = self.scan_payload(&wasm_bytes, &manifest.slug).await?;
        }

        // ⑤ 审批卡（纯声明式：wasm 摘要为零值 + ui/flow 计数留痕）
        let review = InstallReview {
            slug: manifest.slug.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            kind: manifest.kind.as_str().to_string(),
            trust_state: trust_state.as_str().to_string(),
            trust_level: outcome.trust_level.clone().unwrap_or_default(),
            signer_key: outcome.public_key.chars().take(16).collect(),
            egress: manifest.permissions.egress.clone(),
            x_secret: manifest.permissions.x_secret.clone(),
            wasm_bytes: wasm_len,
            wasm_sha256: actual_hash,
            ui_pages: manifest.ui.pages.len(),
            flow_templates: manifest.flow.templates.len(),
            connectors: bundled_connector
                .as_ref()
                .map(|(id, _)| vec![id.clone()])
                .unwrap_or_default(),
            limits_ignored: self.manager.limits().tighten_with(&manifest.limits).1,
            source_dir: source_dir.display().to_string(),
        };
        let approval = self.approver.approve(&review).await;
        match approval {
            Ok(true) => {}
            outcome @ (Ok(false) | Err(_)) => {
                // Err(e) = 拒绝附言 / 审批通道故障——透传进错误面（toast/CLI
                // 都要能看到「为什么」），不做哑拒绝。日志留档同源。
                let note = match &outcome {
                    Err(e) if !e.trim().is_empty() => format!("（{e}）"),
                    _ => String::new(),
                };
                tracing::warn!(slug = %manifest.slug, note = %note, "[WasmPlugin] 安装被审批面否决");
                return Err(PluginError::Rejected(format!(
                    "install rejected: {}{note}",
                    manifest.slug
                )));
            }
        }

        // ⑥ 编译 + 元数据对账（纯声明式跳过——无组件；component=0 无效哨兵）
        let (component, tool_meta, policy_meta, step_meta) = if declarative {
            (0u64, None, None, None)
        } else {
            let component = self.manager.runtime().compile_component(&wasm_bytes)?;
            let (tool_meta, policy_meta, step_meta) = self
                .probe_and_reconcile(&manifest, component, trust_state)
                .await?;
            (component, tool_meta, policy_meta, step_meta)
        };

        // ⑥.5 connector 装配自动注册（域侧批）：包内
        // connector.toml → `<workspace>/connectors/<id>/`（落文件即注册，
        // scan_connectors 目录扫描制）。冲突语义：目标在场且内容不同 =
        // **不覆盖** + WARN（connector 含 base/clone_url 等用户必改字段，
        // 覆盖即毁配置）；内容相同 = no-op。复制 IO 失败 = WARN 非致命
        // （环境问题回落手动拷贝边界，安装不炸）。只记实际注册成功的 id
        // （lockfile 记账与卸载提示同源）。
        let registered_connectors = match &bundled_connector {
            Some((id, body)) => self
                .register_connector(id, body)
                .into_iter()
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };

        // ⑦ lockfile（lockfile-first：落位之前写——失败 = 零突变干净中止，
        // 优于「报失败但新载荷已生效」的谎报；与 uninstall 的 remove-lockfile
        // 先行对称。load_one 不读 lockfile（sha 自洽比对），此处失败留下的
        // 元数据超前不破本进程装载，重装自愈。 复查 #5）
        self.update_lockfile(&manifest, trust_state, &outcome, &registered_connectors)?;

        // ⑧ 落位（升级路径：先清旧载荷；数据目录保留不动）。删除重试耗尽
        // 必须中止——继续落位会新旧载荷文件混布（交付审查）。
        let target = self.manager.plugin_dir(&manifest.slug);
        match resilient_remove_dir_all(&target) {
            Ok(true) => {
                tracing::info!(slug = %manifest.slug, "[WasmPlugin] 升级前清理旧载荷完成");
            }
            Ok(false) => {}
            Err(e) => {
                return Err(PluginError::Io(format!(
                    "升级前清理旧载荷失败，中止落位（保留现场）: {e}"
                )));
            }
        }
        std::fs::create_dir_all(&target)
            .map_err(|e| PluginError::Io(format!("create plugin dir: {e}")))?;
        std::fs::write(target.join("plugin.toml"), raw.as_bytes())
            .map_err(|e| PluginError::Io(format!("write manifest: {e}")))?;
        if !declarative {
            std::fs::write(target.join(&manifest.wasm), &wasm_bytes)
                .map_err(|e| PluginError::Io(format!("write wasm: {e}")))?;
        }

        if let Some(note) = scan_note {
            tracing::info!(slug = %manifest.slug, "[WasmPlugin] {note}");
        }
        Ok(ApprovedInstall {
            manifest,
            trust_state,
            component,
            tool_meta,
            policy_meta,
            step_meta,
            connectors: registered_connectors,
        })
    }

    /// ①.5 包内 connector.toml 预检：缺席 = None；解析失败 = fail-closed
    /// 拒装（Manifest 错，零突变）。返回 (connector id, 原文)。
    fn parse_bundled_connector(
        &self,
        source_dir: &Path,
    ) -> Result<Option<(String, String)>, PluginError> {
        let path = source_dir.join(nemesis_connectors::registry::MANIFEST_FILE);
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(PluginError::Manifest(format!(
                    "read {}: {e}",
                    path.display()
                )));
            }
        };
        let validated = nemesis_connectors::manifest::parse_connector_manifest(&raw)
            .map_err(|e| PluginError::Manifest(format!("connector.toml 解析失败: {e}")))?;
        Ok(Some((validated.id, raw)))
    }

    /// ⑥.5 connector 注册（单条）：见 install_core 内注释。返回注册成功
    /// 的 id（冲突不覆盖/IO 失败 = None）。
    fn register_connector(&self, id: &str, body: &str) -> Option<String> {
        let dir = self
            .manager
            .workspace_root()
            .join(nemesis_connectors::registry::CONNECTORS_DIR)
            .join(id);
        let target = dir.join(nemesis_connectors::registry::MANIFEST_FILE);
        if let Ok(existing) = std::fs::read_to_string(&target) {
            if existing == body {
                tracing::info!(
                    connector = id,
                    "[WasmPlugin] connector 已在册且内容一致（no-op）"
                );
                return Some(id.to_string());
            }
            tracing::warn!(
                connector = id,
                target = %target.display(),
                "[WasmPlugin] connector 已存在且内容不同，不覆盖（保留用户自定义；插件侧未注册该 connector，需手动合并）"
            );
            return None;
        }
        if std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&target, body))
            .is_err()
        {
            tracing::warn!(
                connector = id,
                target = %target.display(),
                "[WasmPlugin] connector 注册失败（复制 IO 错误；回落手动拷贝边界，安装不受影响）"
            );
            return None;
        }
        tracing::info!(
            connector = id,
            "[WasmPlugin] connector 已注册（装配自动注册）"
        );
        Some(id.to_string())
    }

    /// 病毒扫描（第④步）；Ok(Some(注记))=跳过，Err=拦截。
    async fn scan_payload(
        &self,
        wasm_bytes: &[u8],
        slug: &str,
    ) -> Result<Option<String>, PluginError> {
        let Some(chain) = self.scanner.as_ref() else {
            return Ok(Some(
                "病毒扫描跳过：无已启用引擎（第 7 层缺席，其余闸照常）".to_string(),
            ));
        };
        let chain = chain.read().await;
        if !chain.is_enabled() || chain.engine_count() == 0 {
            return Ok(Some(
                "病毒扫描跳过：无已启用引擎（第 7 层缺席，其余闸照常）".to_string(),
            ));
        }
        let result = chain.scan_content(wasm_bytes).await;
        if result.blocked {
            return Err(PluginError::Scan(format!(
                "virus scan blocked plugin {slug}: engine={} threat={}",
                result.engine, result.virus
            )));
        }
        Ok(None)
    }

    /// 编译后对账（第⑥步）：tool = probe get-metadata + 字段合法性；
    /// observer = 只编译验证（probe 由注册后首事件兜底）；
    /// policy = probe get-policy-metadata + ≥1 接管点校验；
    /// panel = 探针 get-step-metadata 分诊（组合 world 批）——
    /// 成功 = panel 双导出 flow-step 的组合形态（按 flow-step 同规对账
    /// kinds）；壳明报「not a flow-step plugin」= 纯面板（无运行期元数据；
    /// 页面/路由合法性是声明面，归 manifest validate）——锚点是壳+宿主
    /// 同仓发运的确定性错误文案（host_dll_freshness 测试钉壳陈旧）；
    /// 其余错误原样上抛（真故障不吞）；
    /// flow-step = probe get-step-metadata + kind 词表非空 + 形态校验
    /// （guest step 批——运行期路由按此对账）。
    #[allow(clippy::type_complexity)]
    async fn probe_and_reconcile(
        &self,
        manifest: &PluginManifest,
        component: u64,
        trust_state: PluginTrustState,
    ) -> Result<
        (
            Option<ToolMetaSnapshot>,
            Option<PolicyMetaSnapshot>,
            Option<StepMetaSnapshot>,
        ),
        PluginError,
    > {
        let _ = trust_state;
        if *manifest.kind == PluginKind::Observer {
            return Ok((None, None, None));
        }
        if *manifest.kind == PluginKind::Policy {
            let frame = probe_frame(self.manager.as_ref(), manifest);
            let meta = self
                .manager
                .runtime()
                .probe_policy_metadata(component, frame)
                .await?;
            let pm = PolicyMetaSnapshot {
                compact: meta.compact,
                final_judge: meta.final_judge,
                validation: meta.validation,
            };
            if pm.points().is_empty() {
                return Err(PluginError::MetadataMismatch(
                    "policy component must take over at least one policy point \
                     (compact | final-judge | validation)"
                        .into(),
                ));
            }
            return Ok((None, Some(pm), None));
        }
        // panel 分诊（组合 world 批）：kind=panel 组件可能是
        // panel 双导出 flow-step 的组合形态——探针 get-step-metadata 试探。
        // 成功 = 组合（快照进注册表，run_flow_step 按 flow-step 同规投递）；
        // 壳「not a flow-step plugin」明报 = 纯面板（无步骤面，原样放行）；
        // 其余 Err 原样上抛（真故障不吞——组合组件自报坏词表也必须装失败）。
        if *manifest.kind == PluginKind::Panel {
            let frame = probe_frame(self.manager.as_ref(), manifest);
            match self
                .manager
                .runtime()
                .probe_step_metadata(component, frame)
                .await
            {
                Ok(meta) => {
                    validate_step_kinds(&meta.kinds)?;
                    return Ok((None, None, Some(StepMetaSnapshot { kinds: meta.kinds })));
                }
                Err(PluginError::Host(ref msg)) if msg.contains("not a flow-step plugin") => {
                    return Ok((None, None, None));
                }
                Err(e) => return Err(e),
            }
        }
        // flow 步骤对账（guest step 批）：探针 get-step-metadata
        // （紧超时同 validation 口径）；kind 词表非空 + 形态校验（1-64 字符
        // 无空白/控制字符）——模板 `plugin:<slug>:<op>` 引用的 op 必须在词
        // 表内，安装期就把越权引用拦在门外。
        if *manifest.kind == PluginKind::FlowStep {
            let frame = probe_frame(self.manager.as_ref(), manifest);
            let meta = self
                .manager
                .runtime()
                .probe_step_metadata(component, frame)
                .await?;
            validate_step_kinds(&meta.kinds)?;
            return Ok((None, None, Some(StepMetaSnapshot { kinds: meta.kinds })));
        }
        let frame = probe_frame(self.manager.as_ref(), manifest);
        let meta = self
            .manager
            .runtime()
            .probe_metadata(component, frame)
            .await?;
        // 对账：基础名合法（slug 同规）；参数 JSON 可解析且顶层 object；
        // operation-type 词表收敛（未知值从紧 = 空串语义，宿主按未注册名
        // 默认 CRITICAL——这里直接拒绝未知词，避免安装时看不出笔误）。
        if !valid_slug(&meta.name) {
            return Err(PluginError::MetadataMismatch(format!(
                "tool name '{}' invalid (lowercase [a-z0-9-])",
                meta.name
            )));
        }
        let schema: serde_json::Value =
            serde_json::from_str(&meta.parameters_json).map_err(|e| {
                PluginError::MetadataMismatch(format!("parameters-json not valid JSON: {e}"))
            })?;
        if !schema.is_object() {
            return Err(PluginError::MetadataMismatch(
                "parameters-json must be a JSON object (top-level)".into(),
            ));
        }
        if !matches!(
            meta.operation_type.as_str(),
            "" | "read" | "write" | "exec" | "network"
        ) {
            return Err(PluginError::MetadataMismatch(format!(
                "operation-type '{}' invalid (read|write|exec|network|empty)",
                meta.operation_type
            )));
        }
        Ok((
            Some(ToolMetaSnapshot {
                name: format!(
                    "{}{}.{}",
                    crate::PLUGIN_TOOL_PREFIX,
                    manifest.slug,
                    meta.name
                ),
                base: meta.name.clone(),
                title: meta.title,
                description: meta.description,
                parameters_json: meta.parameters_json,
                operation_type: meta.operation_type,
                min_tier: manifest.min_tier.clone(),
            }),
            None,
            None,
        ))
    }

    fn update_lockfile(
        &self,
        manifest: &PluginManifest,
        trust_state: PluginTrustState,
        outcome: &VerificationOutcome,
        connectors: &[String],
    ) -> Result<(), PluginError> {
        let path = self.manager.plugins_dir().join("lockfile.json");
        // 文件缺失 = 全新 lockfile；损坏 = 诚实报错（静默重置会丢全部安装
        // 历史， 交付审查）。
        let mut lockfile: PluginLockfile = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| {
                PluginError::Io(format!("lockfile 解析失败 {}: {e}", path.display()))
            })?,
            Err(_) => PluginLockfile {
                version: 1,
                plugins: Default::default(),
            },
        };
        lockfile.version = 1;
        lockfile.plugins.insert(
            manifest.slug.clone(),
            LockfileEntry {
                version: manifest.version.clone(),
                wasm_sha256: manifest.wasm_sha256.clone().unwrap_or_default(),
                trust: trust_state.as_str().to_string(),
                installed_at_ms: unix_millis(),
                signed_by: outcome.public_key.chars().take(16).collect(),
                connectors: connectors.to_vec(),
            },
        );
        let body = serde_json::to_string_pretty(&lockfile)
            .map_err(|e| PluginError::Io(format!("lockfile serialize: {e}")))?;
        crate::registry::atomic_write(&path, body.as_bytes())
            .map_err(|e| PluginError::Io(format!("write lockfile: {e}")))
    }

    /// 启动装载（无审批/无扫描——安装期已裁决）。新纪律：
    /// 发现 ≠ 加载——扫全部含 `plugin.toml` 的子目录得发现清单，仅
    /// `config.plugin.wasm.json` 记录 `enabled=true` 者装载。返回加载数。
    pub async fn load_all(&self) -> usize {
        let dir = self.manager.plugins_dir();
        let mut slugs: Vec<String> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().join("plugin.toml").is_file())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect(),
            Err(_) => Vec::new(),
        };
        slugs.sort(); // 确定性装载顺序（read_dir 顺序是平台实现细节）
        let config_dir = self.manager.workspace_root().join("config");
        // 读路径触发旧载荷/旧实例文件惰性迁移（enabled 强制 false）。
        let switches = nemesis_config::plugin_wasm::load_switches(&config_dir);
        let enabled: Vec<String> = slugs
            .iter()
            .filter(|s| switches.get(s.as_str()).is_some_and(|sw| sw.enabled))
            .cloned()
            .collect();
        let mut loaded = 0usize;
        for slug in &enabled {
            let pdir = dir.join(slug);
            match self
                .load_one(&pdir, self.recorded_allow_unsigned(slug))
                .await
            {
                Ok(_) => loaded += 1,
                Err(e) => tracing::warn!(
                    slug = %slug,
                    error = %e,
                    "[WasmPlugin] 启动装载失败（开关保持开启——修复后重启重试；插件文件保留在盘上）"
                ),
            }
        }
        tracing::info!(
            dir = %dir.display(),
            "[WasmPlugin] 启动装载：发现 {} 个插件，加载 {} 个（其余开关关闭）",
            slugs.len(),
            loaded
        );
        loaded
    }

    /// 加载开关（Dashboard / CLI enable|disable 共用；漏斗锁内）。
    ///
    /// on：已注册 = 幂等（只写开关）；载荷目录在场 → 写开关 true 后
    /// `load_one` 装配——lockfile 有记录走原路；无记录（手工拷入）先补跑
    /// 完整漏斗（含审批卡）。无签名载荷的容忍判定见
    /// [`Self::hand_copied_unsigned_tolerable`]：审批面具备人批卡能力
    /// （gateway 卡面）且仅「无签名」→ 放宽进漏斗由卡裁决；签名失效/
    /// 吊销或无卡面（CLI）维持从紧。开关先落盘：load 失败开关保持
    /// true = 诚实可见态（重启装载再试）。
    ///
    /// off：摘除注册 → 写开关 false（顺序防「盘上关了、内存还活着」）。
    pub async fn set_enabled(
        &self,
        slug: &str,
        on: bool,
    ) -> Result<Option<Arc<RegisteredPlugin>>, PluginError> {
        if !crate::manifest::valid_slug(slug) {
            return Err(PluginError::Manifest(format!("invalid slug: {slug}")));
        }
        let _funnel = FUNNEL_LOCK.lock().await;
        let config_dir = self.manager.workspace_root().join("config");
        if !on {
            self.manager.unregister(slug);
            nemesis_config::plugin_wasm::set_enabled(&config_dir, slug, false)
                .map_err(PluginError::Io)?;
            tracing::info!(slug, "[WasmPlugin] 已停用（注册摘除 + 开关关闭）");
            return Ok(None);
        }
        // on：已注册 = 幂等。
        if let Some(reg) = self.manager.get(slug) {
            nemesis_config::plugin_wasm::set_enabled(&config_dir, slug, true)
                .map_err(PluginError::Io)?;
            return Ok(Some(reg));
        }
        let pdir = self.manager.plugin_dir(slug);
        if !pdir.join("plugin.toml").is_file() {
            return Err(PluginError::Manifest(format!(
                "plugin '{slug}' not found in {}（安装或手工拷入后重试）",
                self.manager.plugins_dir().display()
            )));
        }
        // 开关先落盘（load 失败保持 true = 重启重试的诚实态）。
        nemesis_config::plugin_wasm::set_enabled(&config_dir, slug, true)
            .map_err(PluginError::Io)?;
        // lockfile 无记录 = 手工拷入：补跑完整装配漏斗（含审批卡）。
        let lockfile_has = self
            .read_lockfile()
            .map(|l| l.plugins.contains_key(slug))
            .unwrap_or(false);
        if !lockfile_has {
            tracing::info!(
                slug,
                "[WasmPlugin] 载荷无安装记录（手工拷入），补跑装配漏斗"
            );
            // 无签名载荷的容忍判定（开关补漏斗收口）：审批面具备
            // 人批卡能力（gateway 审批卡）且载荷仅「无签名」→ 放宽进漏斗——
            // 卡面展示 trust=review-required，批准 = 人显式接受（与
            // `install --allow-unsigned` 同一留档语义，lockfile 记
            // review-required，重启装载沿用同一容差）。签名**失效/吊销**
            // 或无卡面（CLI --yes / headless）维持从紧——卡面救不了失效签名，
            // CLI 走 `install --allow-unsigned` 显式旗标或 `plugin sign`。
            let tolerate_unsigned = self.hand_copied_unsigned_tolerable(&pdir);
            self.install_core(&pdir, tolerate_unsigned).await?;
        }
        let reg = self
            .load_one(&pdir, self.recorded_allow_unsigned(slug))
            .await?;
        tracing::info!(slug, "[WasmPlugin] 已启用并装载");
        Ok(Some(reg))
    }

    /// 读 lockfile（损坏/缺席 = None——enable 补漏斗判定从紧）。
    fn read_lockfile(&self) -> Option<PluginLockfile> {
        let path = self.manager.plugins_dir().join("lockfile.json");
        let s = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&s).ok()
    }

    /// 手工拷入载荷的「无签名容忍」判定：审批面具备人批卡能力
    ///（[`InstallApprover::can_approve_unsigned`]）且验签结论为**仅无签名**
    ///（失效/吊销不在此列——卡面救不了失效签名，`allow_unsigned` 纪律
    /// 见 crate 文档）→ true。
    fn hand_copied_unsigned_tolerable(&self, pdir: &Path) -> bool {
        if !self.approver.can_approve_unsigned() {
            return false;
        }
        let Ok(raw) = std::fs::read_to_string(pdir.join("plugin.toml")) else {
            return false;
        };
        matches!(
            crate::trust::verify_manifest(&raw, &self.verifier),
            Ok((_, outcome)) if !outcome.signed
        )
    }

    /// 装载容差 = 安装期裁决（lockfile 记录的四态）：装的时候豁免了「无签名」
    ///（ReviewRequired），enable/重启装载沿用同一容差——开关是用户显式意志，
    /// 不该被「装载期比安装期更严」卡死。签名**无效/被吊销**照旧拒绝
    ///（容差只豁免「缺席」，不豁免「失效」）。lockfile 无记录 = 从紧 false。
    fn recorded_allow_unsigned(&self, slug: &str) -> bool {
        self.read_lockfile()
            .and_then(|l| l.plugins.get(slug).map(|e| e.trust.clone()))
            .is_some_and(|t| t == PluginTrustState::ReviewRequired.as_str())
    }

    /// 单插件装载（manifest + 验签 + sha + 编译 + 对账 + 注册）。
    async fn load_one(
        &self,
        pdir: &Path,
        allow_unsigned: bool,
    ) -> Result<Arc<crate::registry::RegisteredPlugin>, PluginError> {
        let raw = std::fs::read_to_string(pdir.join("plugin.toml"))
            .map_err(|e| PluginError::Manifest(format!("read plugin.toml: {e}")))?;
        let (manifest, outcome) = verify_manifest(&raw, &self.verifier)?;
        // 目录名与 manifest.slug 不一致 = 孤儿载荷（注册用 manifest.slug、
        // 卸载/升级只操作 canonical 目录名——错位目录每次启动都会重装成
        // 幽灵注册项； 交付审查）。
        let dir_name = pdir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if dir_name != manifest.slug {
            return Err(PluginError::Manifest(format!(
                "目录名 {dir_name:?} 与 manifest slug {:?} 不一致（孤儿载荷，请清理或重装）",
                manifest.slug
            )));
        }
        // 装载容差由调用方按 lockfile 安装期记录给（recorded_allow_unsigned）。
        let trust_state = trust_state_for(&outcome, allow_unsigned)?;
        // 纯声明式（无载荷）：跳过 sha/编译/对账——无字节可比对、无组件
        // 可编译；component=0 无效哨兵（壳对 0 容忍；invoke 侧对 0 诚实
        // 报「无载荷」）。
        if manifest.wasm_sha256.is_none() {
            return self
                .manager
                .register(manifest, trust_state, 0, None, None, None);
        }
        ensure_plain_filename(&manifest.wasm)?;
        let wasm_path = pdir.join(&manifest.wasm);
        let wasm_bytes = std::fs::read(&wasm_path)
            .map_err(|e| PluginError::Wasm(format!("read {}: {e}", wasm_path.display())))?;
        let actual_hash = hex::encode(Sha256::digest(&wasm_bytes));
        let expect = manifest.wasm_sha256.as_deref().unwrap_or_default();
        if actual_hash != expect.to_lowercase() {
            return Err(PluginError::Wasm(format!(
                "wasm sha256 mismatch (payload changed after install?): manifest {expect} actual {actual_hash}",
            )));
        }
        let component = self.manager.runtime().compile_component(&wasm_bytes)?;
        let (tool_meta, policy_meta, step_meta) = self
            .probe_and_reconcile(&manifest, component, trust_state)
            .await?;
        self.manager.register(
            manifest,
            trust_state,
            component,
            tool_meta,
            policy_meta,
            step_meta,
        )
    }
}

/// step kinds 词表对账（组合 world 批从 probe_and_reconcile 提取共用）：
/// 非空 + 形态校验（1-64 字符无空白/控制字符）——模板 `plugin:<slug>:<op>`
/// 引用的 op 必须在词表内，安装期就把越权引用拦在门外。
fn validate_step_kinds(kinds: &[String]) -> Result<(), PluginError> {
    if kinds.is_empty() {
        return Err(PluginError::MetadataMismatch(
            "flow-step component must report at least one step kind".into(),
        ));
    }
    for k in kinds {
        if !crate::manifest::valid_step_op(k) {
            return Err(PluginError::MetadataMismatch(format!(
                "invalid step kind '{k}' (1-64 chars, no whitespace/control)"
            )));
        }
    }
    Ok(())
}

/// 探针帧（安装/装载期；invoker/secrets 不注入——元数据自报不该碰能力面）。
fn probe_frame(manager: &PluginManager, manifest: &PluginManifest) -> crate::runtime::CallFrame {
    // 探针发生在 register 之前（数据目录此时尚未创建）；guest 组件可能带
    // wasi:cli/filesystem 导入，fresh_store 的 preopen 需要目录在场。
    let data_dir = manager.plugin_data_dir(&manifest.slug);
    let _ = std::fs::create_dir_all(&data_dir);
    // 探针帧同样吃 manifest 收紧表（与 register 后的执行路径同源——
    // 恶意载荷在 probe 阶段就该被同一套限制约束）。
    let (limits, _) = manager.limits().tighten_with(&manifest.limits);
    crate::runtime::CallFrame {
        observer_frame: false,
        slug: manifest.slug.clone(),
        kind: *manifest.kind,
        limits,
        logs: Arc::new(crate::host_impl::PluginLogBuffer::new(
            manager.limits().log_ring_capacity,
            manager.limits().log_line_max_bytes,
        )),
        config: Arc::new(crate::host_impl::InstanceConfig::default()),
        secrets: Arc::new(NoSecrets),
        workspace_root: manager.workspace_root().to_path_buf(),
        egress: Arc::new(crate::egress::EgressPolicy::default()),
        invoker: None,
        llm: None,
        secret_write: None,
        config_write: None,
        data_dir,
        audit: None,
        session_key: String::new(),
    }
}

/// 无凭据解析器（CLI / 探针 / 测试形态——manifest x-secret 一律解析失败
/// 诚实回 None）。
pub struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Windows 8.3 短名段识别（`ABCDEF~1` / `ABCDEF~1.HTM` 形态；含 `~数字`
/// 尾巴即拒绝——canonicalize 家族对短名的展开是环境相关行为，不赌）。
#[must_use]
pub fn is_eight_short_name(seg: &str) -> bool {
    let Some((base, tail)) = seg.split_once('~') else {
        return false;
    };
    !base.is_empty() && tail.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// 韧性删除（Windows AV/索引器瞬时句柄是常态：80ms×10 退避重试）。
/// `Ok(false)` = 目录不存在（首装/已删，正常）；`Ok(true)` = 已删除；
/// `Err` = 重试耗尽删除失败——调用方必须中止，不能与「不存在」混同
///（交付审查）。
pub fn resilient_remove_dir_all(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let mut last_err = None;
    for attempt in 0..10u32 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(true),
            Err(e) if attempt < 9 => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
            Err(e) => last_err = Some(e),
        }
    }
    let msg = last_err.map(|e| e.to_string()).unwrap_or_default();
    tracing::warn!(
        path = %path.display(),
        error = %msg,
        "[WasmPlugin] 目录删除重试耗尽（Windows 句柄占用？）"
    );
    Err(msg)
}

/// wasm 载荷文件名必须为纯文件名（防 join 逃逸：`../`、绝对路径、`C:foo`
/// 盘符相对形态——`file_name()` 归一化比较一次覆盖全部形态；join 绝对路径
/// 时整体替换，可把载荷读到源目录之外。 交付审查）。
fn ensure_plain_filename(name: &str) -> Result<(), PluginError> {
    if name.is_empty() || std::path::Path::new(name).file_name() != Some(std::ffi::OsStr::new(name))
    {
        return Err(PluginError::Wasm(format!(
            "wasm 载荷必须为纯文件名（不含路径分隔符/..），got: {name:?}"
        )));
    }
    Ok(())
}

/// lockfile 摘除前的条目 connector 清单（域侧批：卸载提示
/// 取证；读失败/条目缺席 = 空表，不阻塞卸载）。
fn lockfile_connectors_of(manager: &PluginManager, slug: &str) -> Vec<String> {
    let path = manager.plugins_dir().join("lockfile.json");
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(lf) = serde_json::from_str::<PluginLockfile>(&s) else {
        return Vec::new();
    };
    lf.plugins
        .get(slug)
        .map(|e| e.connectors.clone())
        .unwrap_or_default()
}

/// lockfile 摘除单条目（原子写；漏斗锁内调用）。返回是否原本在场。
fn remove_lockfile_entry(manager: &PluginManager, slug: &str) -> Result<bool, PluginError> {
    let path = manager.plugins_dir().join("lockfile.json");
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut lf: PluginLockfile = serde_json::from_str(&s)
        .map_err(|e| PluginError::Io(format!("lockfile 解析失败 {}: {e}", path.display())))?;
    if lf.plugins.remove(slug).is_none() {
        return Ok(false);
    }
    let body = serde_json::to_string_pretty(&lf)
        .map_err(|e| PluginError::Io(format!("lockfile serialize: {e}")))?;
    crate::registry::atomic_write(&path, body.as_bytes())
        .map_err(|e| PluginError::Io(format!("write lockfile: {e}")))?;
    Ok(true)
}

/// 卸载（漏斗互斥内执行）。lockfile 摘除 → 注销 + 删载荷。数据目录与实例
/// 配置文件保留（操作员手动清理）。lockfile 摘除先行且失败即中止——反序会
/// 出现「载荷已删而 lockfile 留条目」的漂移态（交付审查 /3.4）。
pub async fn uninstall(manager: &PluginManager, slug: &str) -> Result<bool, PluginError> {
    if !valid_slug(slug) {
        return Err(PluginError::Manifest(format!("invalid slug: {slug}")));
    }
    let _funnel = FUNNEL_LOCK.lock().await;
    uninstall_inner(manager, slug)
}

/// 卸载本体（同步，漏斗锁由调用方持有）。`uninstall_blocking` 供无异步
/// 上下文的 Drop 尽力清理使用（测试收尾单线程，无并发面）。
fn uninstall_inner(manager: &PluginManager, slug: &str) -> Result<bool, PluginError> {
    // 卸载提示取证先于摘除（摘了就查无此条）。connector **不随卸载删除**
    // ——可能已被用户定制（base/clone_url 等），删除即毁配置。
    let connectors = lockfile_connectors_of(manager, slug);
    let in_lockfile = remove_lockfile_entry(manager, slug)?;
    let existed = manager.unregister(slug);
    let removed = resilient_remove_dir_all(&manager.plugin_dir(slug)).map_err(|e| {
        PluginError::Io(format!("删除载荷目录失败（lockfile 已摘除，可重试）: {e}"))
    })?;
    // 开关记录一并摘除（幂等；失败不阻塞卸载——孤儿条目无害，重装覆盖）。
    let config_dir = manager.workspace_root().join("config");
    let _ = nemesis_config::plugin_wasm::remove(&config_dir, slug);
    // 卸载留痕（日志面收口）：此前未装载插件的卸载完全静默
    //（已装载的至少有工具面 hot-unregistered 日志）——三清点一行说清。
    if existed || removed || in_lockfile {
        tracing::info!(
            slug,
            registered = existed,
            payload_removed = removed,
            lockfile_removed = in_lockfile,
            "[WasmPlugin] 已卸载（载荷/lockfile/开关记录清点）"
        );
    }
    if !connectors.is_empty() {
        tracing::info!(
            slug,
            connectors = ?connectors,
            "[WasmPlugin] 插件曾注册 connector，未随卸载删除（可能已被定制；需手动清理走 <workspace>/connectors/<id>/）"
        );
    }
    Ok(existed || removed || in_lockfile)
}

/// 卸载的同步尽力形态（Drop 清理专用——无异步上下文拿不到漏斗锁；正式
/// 路径一律走 [`uninstall`]）。
pub fn uninstall_blocking(manager: &PluginManager, slug: &str) -> Result<bool, PluginError> {
    if !valid_slug(slug) {
        return Err(PluginError::Manifest(format!("invalid slug: {slug}")));
    }
    uninstall_inner(manager, slug)
}

#[cfg(test)]
mod tests;
