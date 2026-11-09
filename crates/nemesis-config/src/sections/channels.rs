// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub dm_scope: String,
    #[serde(default)]
    pub identity_links: std::collections::HashMap<String, Vec<String>>,
}

// ============================================================================
// Channels Config
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub whatsapp: WhatsAppConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub feishu: FeishuConfig,
    #[serde(default)]
    pub discord: DiscordConfig,
    #[serde(default)]
    pub maixcam: MaixCamConfig,
    #[serde(default)]
    pub qq: QqConfig,
    #[serde(default)]
    pub dingtalk: DingTalkConfig,
    #[serde(default)]
    pub slack: SlackConfig,
    #[serde(default)]
    pub line: LineConfig,
    #[serde(default)]
    pub onebot: OneBotConfig,
    #[serde(default)]
    pub web: WebChannelConfig,
    #[serde(default)]
    pub websocket: WebSocketChannelConfig,
    #[serde(default)]
    pub external: ExternalConfig,
    // Wave 3（gateway 集成面）：存量 feature-gated 通道补缺 + 五新通道。
    // 字段对照 nemesis-channels 侧各通道 Config（真相源在各通道 mod 顶部），
    // gateway web_init 逐字段转换到 ChannelInitConfig。
    #[serde(default)]
    pub matrix: MatrixConfig,
    #[serde(default)]
    pub irc: IrcConfig,
    #[serde(default)]
    pub signal: SignalConfig,
    #[serde(default)]
    pub mastodon: MastodonConfig,
    #[serde(default)]
    pub bluesky: BlueskyConfig,
    #[serde(default)]
    pub wecom: WeComConfig,
    #[serde(default)]
    pub mattermost: MattermostConfig,
    #[serde(default)]
    pub nostr: NostrConfig,
    #[serde(default)]
    pub mqtt: MqttConfig,
    #[serde(default)]
    pub wechat: WeChatConfig,
}

// Channel configuration structs follow below.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bridge_url: String,
    /// 桥 API key（出站鉴权凭据； 补齐——通道侧 `api_key: Option<String>`
    /// 此前无 config 出口，装配点空串 → None）。支持 vault/env/yaml 引用。
    #[serde(default)]
    pub api_key: String,
    /// 出站代理 URL（桥可部署在远端，与 telegram 同构；空 = 直连）。
    /// 模板早已声明 `channels.whatsapp.proxy`，typed 字段
    /// 缺失 → 读取端静默忽略、typed save 静默删除。
    #[serde(default)]
    pub proxy: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub proxy: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default)]
    pub encrypt_key: String,
    #[serde(default)]
    pub verification_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaixCamConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_maixcam_host")]
    pub host: String,
    #[serde(default = "default_maixcam_port")]
    pub port: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

fn default_maixcam_host() -> String {
    "0.0.0.0".to_string()
}

/// 端口缺省 8888（BUG-20261108-010）：与出厂模板 / migrate 模板 / 硬编码
/// `default_config()` / 通道侧 `nemesis_channels::maixcam::MaixCamConfig`
/// 类型缺省四处真相源收敛——旧形态 serde 缺省 0（derive 零值），存量
/// config（channels 段在场面 maixcam 键缺席）使能即
/// `TcpListener::bind("0.0.0.0:0")` 成功落 OS 临时端口：bind 成功、通道
/// 状态「运行中」、设备永远连不上（静默死通道；BUG-20261107-015 只修了
/// 硬编码路径的撞号，serde 路径分叉漏网）。手写 Default 不用 derive——
/// serde 缺键路径与「段整体缺席」路径必须落同一组值（同 AgentsConfig
/// 手写 Default 纪律）。
fn default_maixcam_port() -> i64 {
    8888
}

impl Default for MaixCamConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_maixcam_host(),
            port: default_maixcam_port(),
            allow_from: Vec::new(),
            sync_to: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QqConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingTalkConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub app_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LineConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub channel_secret: String,
    #[serde(default)]
    pub channel_access_token: String,
    #[serde(default)]
    pub webhook_host: String,
    #[serde(default)]
    pub webhook_port: i64,
    #[serde(default)]
    pub webhook_path: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OneBotConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub ws_url: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub reconnect_interval: i64,
    #[serde(default)]
    pub group_trigger_prefix: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WebChannelConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_host")]
    pub host: String,
    // flexible_i64: 历史 CLI 把端口写成字符串，容忍两种磁盘形态
    #[serde(
        default = "default_web_port",
        deserialize_with = "deserialize_flexible_i64"
    )]
    pub port: i64,
    #[serde(default = "default_web_path")]
    pub path: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: i64,
    #[serde(default = "default_session_timeout")]
    pub session_timeout: i64,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

// SEC-002（审查）：手写 Debug 替代 derive——auth_token 一律脱敏
// （Debug/panic/错误上下文会把整个 config 打进日志）。字段名保留，值只输出
// 形态；其余字段与 derive 输出一致。
impl std::fmt::Debug for WebChannelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebChannelConfig")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path", &self.path)
            .field(
                "auth_token",
                &if self.auth_token.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("allow_from", &self.allow_from)
            .field("heartbeat_interval", &self.heartbeat_interval)
            .field("session_timeout", &self.session_timeout)
            .field("sync_to", &self.sync_to)
            .finish()
    }
}

impl Default for WebChannelConfig {
    fn default() -> Self {
        Self {
            // 与字段级 `#[serde(default = "default_true")]` 收敛（serde 双路径
            // 纪律：web 键整体缺席走这里、enabled 字段缺席走字段级 default，
            // 两路必须同值——：曾为 false，精简 config.json
            // （channels 段或 web 子段缺席）会静默关掉 dashboard 唯一 UI，
            // 与出厂模板/default_config/migrate 产物的 enabled:true 相反。
            enabled: true,
            host: default_web_host(),
            port: default_web_port(),
            path: default_web_path(),
            auth_token: String::new(),
            allow_from: vec![],
            heartbeat_interval: default_heartbeat_interval(),
            session_timeout: default_session_timeout(),
            sync_to: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSocketChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub host: String,
    // flexible_i64: 历史 CLI 把端口写成字符串，容忍两种磁盘形态
    #[serde(default, deserialize_with = "deserialize_flexible_i64")]
    pub port: i64,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
    #[serde(default)]
    pub sync_to_web: bool,
    #[serde(default)]
    pub web_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub input_exe: String,
    #[serde(default)]
    pub output_exe: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
    #[serde(default)]
    pub sync_to_web: bool,
    #[serde(default)]
    pub web_session_id: String,
}

// feature-gated 通道 config（Wave 3 集成面补齐）
//
// 与 nemesis-channels 侧各通道 Config 字段一一对应（真相源在各通道 mod 顶部
// 的 `pub struct XConfig`）；`enabled` / `sync_to` 是宿主侧路由字段（通道侧
// 无此二字段——enabled 由 gateway init 臂消费，sync_to 由 setup_sync_targets
// 消费）。秘密字段在 config.json 里只存 `vault:` / `env:` / `yaml:` 引用，
// gateway 装配点统一经 resolve_secret_or_empty / resolve_auth_token_or_random
// 解析（见 web_init.rs 各转换臂）。

/// Matrix 通道配置（对应 `nemesis_channels::matrix:MatrixConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Homeserver URL（如 `https://matrix.org`）。
    #[serde(default)]
    pub homeserver: String,
    /// Bot 用户 ID（如 `@bot:matrix.org`）。
    #[serde(default)]
    pub user_id: String,
    /// Access token（出站 API 凭据；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub access_token: String,
    /// 默认房间 ID；空 = 未设（通道侧 `room_id: Option<String>`，装配点
    /// 空串 → None）。
    #[serde(default)]
    pub room_id: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// IRC 通道配置（对应 `nemesis_channels::irc:IRCConfig`）。
/// Default 对齐通道侧（use_tls=true、退避 5/300）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IrcConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 服务器地址（如 `irc.libera.chat:6697`）。
    #[serde(default)]
    pub server: String,
    /// 是否走 TLS（通道侧默认 true）。
    #[serde(default)]
    pub use_tls: bool,
    /// 昵称。
    #[serde(default)]
    pub nick: String,
    /// 服务器密码（出站鉴权凭据；空 = 无，通道侧 `password: Option<String>`）。
    #[serde(default)]
    pub password: String,
    /// 加入的频道（如 `#nemesisbot`）。
    #[serde(default)]
    pub channel: String,
    /// 重连退避基数秒（通道侧默认 5）。
    #[serde(default)]
    pub reconnect_backoff_secs: i64,
    /// 重连退避上限秒（通道侧默认 300）。
    #[serde(default)]
    pub max_reconnect_backoff_secs: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for IrcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: String::new(),
            use_tls: true,
            nick: String::new(),
            password: String::new(),
            channel: String::new(),
            reconnect_backoff_secs: 5,
            max_reconnect_backoff_secs: 300,
            allow_from: vec![],
            sync_to: vec![],
        }
    }
}

/// Signal 通道配置（对应 `nemesis_channels::signal:SignalConfig`，
/// signal-cli-rest-api 桥接形态）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignalConfig {
    #[serde(default)]
    pub enabled: bool,
    /// signal-cli-rest-api 基地址。
    #[serde(default)]
    pub api_url: String,
    /// 本机电话号码（带国家码）。
    #[serde(default)]
    pub phone_number: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// Mastodon 通道配置（对应 `nemesis_channels::mastodon:MastodonConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MastodonConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 实例地址（如 `https://mastodon.social`）。
    #[serde(default)]
    pub server: String,
    /// OAuth access token（出站 API 凭据；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub access_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// Bluesky 通道配置（对应 `nemesis_channels::bluesky:BlueskyConfig`）。
/// Default 对齐通道侧（poll_interval=10）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BlueskyConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 服务器地址（如 `https://bsky.social`）。
    #[serde(default)]
    pub server: String,
    /// 账号 handle（如 `nemesisbot.bsky.social`）。
    #[serde(default)]
    pub handle: String,
    /// App password（出站登录凭据；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub password: String,
    /// DID；空 = 未设（通道侧自动解析，装配点空串 → None）。
    #[serde(default)]
    pub did: String,
    /// 通知轮询间隔秒（通道侧默认 10）。
    #[serde(default)]
    pub poll_interval: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for BlueskyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: String::new(),
            handle: String::new(),
            password: String::new(),
            did: String::new(),
            poll_interval: 10,
            allow_from: vec![],
            sync_to: vec![],
        }
    }
}

/// WeCom（企业微信）通道配置（对应 `nemesis_channels::wecom:WeComConfig`）。
/// 字段默认全空——通道侧 new 对 listen_addr/callback_path 有空值归一。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeComConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 群机器人 webhook 地址（出站默认目标；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub webhook_url: String,
    /// chat_id → 群机器人 webhook 路由表（多群出站；chat_id 命中优先于默认）。
    #[serde(default)]
    pub webhooks: std::collections::HashMap<String, String>,
    /// 回调验签 token（企业微信后台配置；鉴权验证类，支持引用）。
    #[serde(default)]
    pub token: String,
    /// 回调消息加密密钥 EncodingAESKey（43 字符；鉴权验证类，支持引用）。
    #[serde(default)]
    pub encoding_aes_key: String,
    /// 企业微信 corp_id（解密 receiveid 强校验用；空 = 不校验）。
    #[serde(default)]
    pub corp_id: String,
    /// 回调 HTTP 监听地址（空 = 通道侧默认 0.0.0.0:9898）。
    #[serde(default)]
    pub listen_addr: String,
    /// 回调路径（空 = 通道侧默认 /wecom/callback）。
    #[serde(default)]
    pub callback_path: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// Mattermost 通道配置（对应 `nemesis_channels::mattermost:MattermostConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MattermostConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 服务器基地址（如 `https://mattermost.example.com`，http/https）。
    #[serde(default)]
    pub base_url: String,
    /// Bot token（WS 网关鉴权 + REST 出站鉴权共用；vault/env/yaml 解析走
    /// 出站凭据语义 `resolve_secret_or_empty`——缺失不阻断启动，鉴权失败在
    /// users/me / WS challenge 处暴露）。
    #[serde(default)]
    pub bot_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 监听的频道 ID / 名称列表（空 = bot 所在全部频道）。
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub channels: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// nostr 通道配置（对应 `nemesis_channels::nostr:NostrConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NostrConfig {
    #[serde(default)]
    pub enabled: bool,
    /// relay WebSocket 地址列表（`wss://` / `ws://`），至少一条。
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub relays: Vec<String>,
    /// 本机私钥（64-hex）。config.json 只存 `vault:`/`env:`/`yaml:` 引用，
    /// 装配点解析后传明文（nostr.rs 模块头"密钥管理"约定；
    /// 通道侧对未解析引用前缀响亮拒绝）。
    #[serde(default)]
    pub private_key: String,
    /// 发件人 x-only 公钥白名单（64-hex；空 = 放行所有）。
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 断线重连间隔秒（通道侧"0 视作缺省"= 默认 5）。
    #[serde(default)]
    pub reconnect_secs: i64,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// MQTT 订阅 topic filter → 回包 topic 映射条目（对应
/// `nemesis_channels::mqtt:MqttTopicMapping`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MqttTopicMapping {
    /// 订阅的 topic filter（支持 `+`/`#` 通配）。
    #[serde(default)]
    pub topic: String,
    /// 出站回包 topic；空 = 此映射不承接回包。
    #[serde(default)]
    pub reply_topic: String,
}

/// MQTT 通道配置（对应 `nemesis_channels::mqtt:MqttChannelConfig`）。
/// Default 对齐通道侧文档默认值（broker 127.0.0.1:1883、keep-alive 30、
/// clean_session=true、退避 5、QoS 1）——通道侧对 broker_host 空值响亮报错、
/// 端口/keep-alive 原样透传 rumqttc，缺省值必须在 config 层给足。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MqttConfig {
    #[serde(default)]
    pub enabled: bool,
    /// broker 地址（默认 127.0.0.1）。
    #[serde(default)]
    pub broker_host: String,
    /// broker 端口（默认 1883）。
    #[serde(default)]
    pub broker_port: i64,
    /// MQTT client id；空 = 通道侧自动生成 `nemesisbot-<pid>`。
    #[serde(default)]
    pub client_id: String,
    /// 用户名；空 = 匿名连接（支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub username: String,
    /// 密码；空 = 无（支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub password: String,
    /// keep-alive 秒数（默认 30；0 = 关闭保活）。
    #[serde(default)]
    pub keep_alive_secs: i64,
    /// clean session（默认 true）。
    #[serde(default)]
    pub clean_session: bool,
    /// 断线重连退避秒数（默认 5，通道侧下限 1）。
    #[serde(default)]
    pub reconnect_delay_secs: i64,
    /// 订阅与出站统一 QoS（默认 1；0/1/2，>2 通道侧构造期拒绝）。
    #[serde(default)]
    pub qos: i64,
    /// 订阅 + 回包映射表。
    #[serde(default)]
    pub topics: Vec<MqttTopicMapping>,
    /// 全局默认回包 topic；空 = 无默认（映射/学习都缺位时出站报错）。
    #[serde(default)]
    pub default_reply_topic: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 启用 TLS（默认 false = 纯 TCP）。true 时通道侧走 rustls（ring provider）。
    #[serde(default)]
    pub use_tls: bool,
    /// CA 证书 PEM 路径（use_tls=true 必填；自签/私有 PKI 场景）。
    #[serde(default)]
    pub ca_cert_path: String,
    /// 客户端证书 PEM 路径（mTLS；与 client_key_path 成对出现）。
    #[serde(default)]
    pub client_cert_path: String,
    /// 客户端私钥 PEM 路径（mTLS；与 client_cert_path 成对出现）。
    #[serde(default)]
    pub client_key_path: String,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            broker_host: "127.0.0.1".to_string(),
            broker_port: 1883,
            client_id: String::new(),
            username: String::new(),
            password: String::new(),
            keep_alive_secs: 30,
            clean_session: true,
            reconnect_delay_secs: 5,
            qos: 1,
            topics: vec![],
            default_reply_topic: String::new(),
            allow_from: vec![],
            use_tls: false,
            ca_cert_path: String::new(),
            client_cert_path: String::new(),
            client_key_path: String::new(),
            sync_to: vec![],
        }
    }
}

/// 微信个人微信通道配置（iLink Bot API，对应
/// `nemesis_channels::wechat:WeChatConfig`）。Default 对齐通道侧
/// （base_url/send_path/回调监听/签名方案均为通道侧常量——协议形态为
/// 实现假设，真机校准只需要改 config）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WeChatConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 出站 REST 基址（通道侧默认 `https://ilink.bot.weixin.qq.com`，假设值）。
    #[serde(default)]
    pub base_url: String,
    /// 回调鉴权 token（鉴权验证类；必填，通道侧空值拒绝启动；支持引用）。
    #[serde(default)]
    pub token: String,
    /// 出站消息接口路径（通道侧默认 /v1/message/send，假设值）。
    #[serde(default)]
    pub send_path: String,
    /// 回调监听地址（通道侧默认 0.0.0.0:9541）。
    #[serde(default)]
    pub callback_listen_addr: String,
    /// 回调路径（通道侧默认 /wechat/callback）。
    #[serde(default)]
    pub callback_path: String,
    /// 签名方案：`hmac-sha256`（默认）/ `sha1` / `none`（未知值通道侧 loud 拒绝）。
    #[serde(default)]
    pub signature_scheme: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 联系人映射：user_id → 备注别名（命中时写入 metadata `contact_alias`）。
    #[serde(default)]
    pub contacts: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for WeChatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://ilink.bot.weixin.qq.com".to_string(),
            token: String::new(),
            send_path: "/v1/message/send".to_string(),
            callback_listen_addr: "0.0.0.0:9541".to_string(),
            callback_path: "/wechat/callback".to_string(),
            signature_scheme: "hmac-sha256".to_string(),
            allow_from: vec![],
            contacts: std::collections::HashMap::new(),
            sync_to: vec![],
        }
    }
}

// ============================================================================
// Model Config
// ============================================================================
