//! Per-server, per-user, and deployment-wide bot configuration.
//! Production persists to the PostgreSQL `bot_config` table; tests and
//! deployments without a database fall back to JSON files under DATA_DIR.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::fs;

use housebot_config::data_dir;
use housebot_llm::ThinkingMode;

const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";

// ── storage backend ───────────────────────────────────────────────────────────

#[derive(Clone)]
enum Backend {
    Files(PathBuf),
    Postgres(Arc<tokio_postgres::Client>),
}

fn json_path(dir: &std::path::Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

impl Backend {
    /// `Ok(None)` means the key genuinely has no stored value; storage
    /// failures are propagated so callers can avoid resetting to defaults.
    async fn load(&self, name: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match self {
            Backend::Files(dir) => match fs::read(json_path(dir, name)).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            },
            Backend::Postgres(client) => match client
                .query_opt("SELECT value FROM bot_config WHERE key = $1", &[&key])
                .await
            {
                Ok(Some(row)) => Ok(Some(row.get::<_, String>(0).into_bytes())),
                Ok(None) => Ok(None),
                Err(error) => {
                    tracing::error!(%error, key, "failed to load bot config");
                    Err(error.into())
                }
            },
        }
    }

    async fn save(&self, name: &str, key: &str, value: String) -> anyhow::Result<()> {
        match self {
            Backend::Files(dir) => {
                fs::create_dir_all(dir).await?;
                fs::write(json_path(dir, name), value).await?;
            }
            Backend::Postgres(client) => {
                client
                    .execute(
                        "INSERT INTO bot_config (key, value) VALUES ($1, $2) \
                         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
                        &[&key, &value],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn delete(&self, name: &str, key: &str) -> std::io::Result<()> {
        match self {
            Backend::Files(dir) => match fs::remove_file(json_path(dir, name)).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            Backend::Postgres(client) => {
                if let Err(error) = client
                    .execute("DELETE FROM bot_config WHERE key = $1", &[&key])
                    .await
                {
                    tracing::error!(%error, key, "failed to delete bot config");
                }
                Ok(())
            }
        }
    }
}

/// Connect to the deployment's PostgreSQL bot-config storage.
pub async fn postgres_client_from_env() -> anyhow::Result<Arc<tokio_postgres::Client>> {
    let url = housebot_config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
    let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!(%error, "PostgreSQL bot-config connection closed");
        }
    });
    Ok(Arc::new(client))
}

/// One-time, non-destructive import from the former JSON-file backend.
async fn import_legacy_files(client: &tokio_postgres::Client, dir: &Path, key_prefix: &str) {
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(content) = fs::read_to_string(&path).await else {
            continue;
        };
        let key = format!("{key_prefix}{stem}");
        if let Err(error) = client
            .execute(
                "INSERT INTO bot_config (key, value) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&key, &content],
            )
            .await
        {
            tracing::error!(%error, key, "failed to import legacy bot config file");
        }
    }
}

// ── server config ─────────────────────────────────────────────────────────────

/// Configuration scoped to a Discord guild (server).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Channel IDs the bot is allowed to respond in. Empty means all channels.
    #[serde(default)]
    pub allowed_channel_ids: HashSet<u64>,
    /// Who can view the server token leaderboard and whether the response is public.
    #[serde(default)]
    pub leaderboard_visibility: LeaderboardVisibility,
    /// Roles allowed to view the leaderboard when visibility is restricted.
    #[serde(default)]
    pub leaderboard_role_ids: HashSet<u64>,
    /// Whether to respond to @-mentions from other bots in this server.
    /// The bot always ignores its own pings regardless.
    #[serde(default)]
    pub respond_to_bot_pings: bool,
    /// Whether proactive assistance is allowed in this server at all.
    /// Users still opt in individually via `/personalize proactive`.
    #[serde(default = "default_respond")]
    pub proactive_allowed: bool,
    /// Whether embed rendering (Discord link previews and bot embeds) is
    /// allowed in this server. When false, overrides any user-level preference.
    #[serde(default = "default_embed_enabled")]
    pub embed_enabled: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            allowed_channel_ids: HashSet::new(),
            leaderboard_visibility: LeaderboardVisibility::default(),
            leaderboard_role_ids: HashSet::new(),
            respond_to_bot_pings: false,
            proactive_allowed: true,
            embed_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderboardVisibility {
    #[default]
    Public,
    Private,
    Restricted,
}

impl LeaderboardVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Restricted => "restricted",
        }
    }
}

#[derive(Clone)]
pub struct ServerConfigStore {
    backend: Backend,
}

impl Default for ServerConfigStore {
    fn default() -> Self {
        Self::new(data_dir().join("server_config"))
    }
}

impl ServerConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            backend: Backend::Files(dir),
        }
    }

    /// Database-backed store; imports any legacy JSON files once.
    pub async fn postgres(client: Arc<tokio_postgres::Client>) -> Self {
        import_legacy_files(&client, &data_dir().join("server_config"), "server:").await;
        Self {
            backend: Backend::Postgres(client),
        }
    }

    pub async fn load(&self, guild_id: u64) -> ServerConfig {
        let bytes = self
            .backend
            .load(&guild_id.to_string(), &format!("server:{guild_id}"))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub async fn save(&self, guild_id: u64, cfg: &ServerConfig) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(cfg)?;
        self.backend
            .save(&guild_id.to_string(), &format!("server:{guild_id}"), data)
            .await
    }

    /// Returns true if the channel is allowed (or if no restrictions are set).
    pub async fn is_channel_allowed(&self, guild_id: Option<u64>, channel_id: u64) -> bool {
        let Some(gid) = guild_id else {
            return true; // DMs are always allowed
        };
        let cfg = self.load(gid).await;
        cfg.allowed_channel_ids.is_empty() || cfg.allowed_channel_ids.contains(&channel_id)
    }

    /// Follow-ups require an explicitly configured server channel.
    pub async fn is_followup_channel_allowed(
        &self,
        guild_id: Option<u64>,
        channel_id: u64,
    ) -> bool {
        let Some(gid) = guild_id else {
            return false;
        };
        self.load(gid)
            .await
            .allowed_channel_ids
            .contains(&channel_id)
    }
}

// ── user config ───────────────────────────────────────────────────────────────

/// Configuration scoped to an individual Discord user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    /// Optional personality/tone override injected into the system prompt.
    #[serde(default)]
    pub personality: Option<String>,
    /// Whether the bot should reply to follow-up messages without a ping/mention
    /// in guild channels. DMs enable follow-ups by default.
    #[serde(default)]
    pub followup_enabled: bool,
    /// How many seconds the bot will reply without a ping after the last interaction.
    #[serde(default = "default_followup_timeout")]
    pub followup_timeout_secs: u64,
    /// Whether LLM responses are rendered as paginated embeds.
    #[serde(default)]
    pub labs_pagination_enabled: bool,
    /// Reasoning budget used for this user's requests (set with `/effort`).
    #[serde(default)]
    pub thinking_mode: ThinkingMode,
    /// Whether intermediate reasoning, queue, and tool progress is shown in Discord.
    #[serde(default = "default_progress_updates_enabled")]
    pub progress_updates_enabled: bool,
    /// Whether the bot may use `update_memory` and auto-save conversation summaries.
    /// When disabled, short-term conversation history still works normally.
    #[serde(default = "default_deep_memory_enabled")]
    pub deep_memory_enabled: bool,
    /// Whether the bot may respond proactively to messages it wasn't mentioned in.
    /// Only narrow cases are handled (obvious reminder requests, help questions).
    #[serde(default)]
    pub proactive_assistance_enabled: bool,
    /// Whether Discord link previews (embeds) are shown for URLs in bot responses.
    /// Server admins can override this server-wide via `/server-config embeds`.
    #[serde(default = "default_embed_enabled")]
    pub embed_enabled: bool,
}

fn default_followup_timeout() -> u64 {
    housebot_config::env_parse("CONVERSATION_IDLE_TIMEOUT", 300)
}

fn default_deep_memory_enabled() -> bool {
    true
}

fn default_progress_updates_enabled() -> bool {
    true
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            personality: None,
            followup_enabled: false,
            followup_timeout_secs: default_followup_timeout(),
            labs_pagination_enabled: false,
            thinking_mode: ThinkingMode::default(),
            progress_updates_enabled: true,
            deep_memory_enabled: true,
            proactive_assistance_enabled: false,
            embed_enabled: true,
        }
    }
}

#[derive(Clone)]
pub struct UserConfigStore {
    backend: Backend,
}

impl Default for UserConfigStore {
    fn default() -> Self {
        Self::new(data_dir().join("user_config"))
    }
}

impl UserConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            backend: Backend::Files(dir),
        }
    }

    /// Database-backed store; imports any legacy JSON files once.
    pub async fn postgres(client: Arc<tokio_postgres::Client>) -> Self {
        import_legacy_files(&client, &data_dir().join("user_config"), "user:").await;
        Self {
            backend: Backend::Postgres(client),
        }
    }

    pub async fn load(&self, user_id: u64) -> UserConfig {
        let bytes = self
            .backend
            .load(&user_id.to_string(), &format!("user:{user_id}"))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub async fn save(&self, user_id: u64, cfg: &UserConfig) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(cfg)?;
        self.backend
            .save(&user_id.to_string(), &format!("user:{user_id}"), data)
            .await
    }

    pub async fn clear(&self, user_id: u64) -> std::io::Result<()> {
        self.backend
            .delete(&user_id.to_string(), &format!("user:{user_id}"))
            .await
    }
}

// ── access control ────────────────────────────────────────────────────────────

/// Per-user policy set by the bot's configurers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UserPolicy {
    /// Cap on `max_tokens` for this user's completions. `None` means no cap.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// Whether the bot responds to this user's messages at all.
    #[serde(default = "default_respond")]
    pub respond: bool,
}

fn default_respond() -> bool {
    true
}

fn default_embed_enabled() -> bool {
    true
}

impl Default for UserPolicy {
    fn default() -> Self {
        Self {
            max_output_tokens: None,
            respond: true,
        }
    }
}

/// Who may configure the bot, plus the per-user policies they manage.
/// The Discord owner (`OWNER_DISCORD_ID`) is always allowed to configure
/// the bot and is never subject to the respond policy. Server administrators
/// get no implicit access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessControl {
    /// Discord user IDs allowed to configure the bot (in addition to the owner).
    #[serde(default)]
    pub configurer_ids: HashSet<u64>,
    /// Per-user output-token caps and respond flags, keyed by Discord user ID.
    #[serde(default)]
    pub user_policies: HashMap<u64, UserPolicy>,
    /// Global switch for proactive assistance. When false, per-user
    /// `/personalize proactive` settings are ignored for everyone.
    #[serde(default = "default_respond")]
    pub proactive_enabled: bool,
}

impl Default for AccessControl {
    fn default() -> Self {
        Self {
            configurer_ids: HashSet::new(),
            user_policies: HashMap::new(),
            proactive_enabled: true,
        }
    }
}

impl AccessControl {
    pub fn is_configurer(&self, user_id: u64, owner_id: u64) -> bool {
        (owner_id != 0 && user_id == owner_id) || self.configurer_ids.contains(&user_id)
    }

    pub fn policy(&self, user_id: u64) -> UserPolicy {
        self.user_policies
            .get(&user_id)
            .copied()
            .unwrap_or_default()
    }

    /// Configurers (and the owner) can always use the bot regardless of policy.
    pub fn should_respond(&self, user_id: u64, owner_id: u64) -> bool {
        self.is_configurer(user_id, owner_id) || self.policy(user_id).respond
    }
}

const ACCESS_CONTROL_KEY: &str = "access_control";

/// How long a cached access snapshot serves reads before storage is consulted
/// again. In-process saves refresh the cache immediately.
const ACCESS_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct AccessControlStore {
    backend: Backend,
    /// Last successfully loaded snapshot; serves the hot path within the TTL
    /// and preserves the last known policy when storage is unreachable.
    cache: Arc<tokio::sync::RwLock<Option<(AccessControl, Instant)>>>,
    /// Serializes read-modify-write updates so concurrent configuration
    /// changes cannot overwrite each other.
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Default for AccessControlStore {
    fn default() -> Self {
        Self::new(data_dir().join("bot_config"))
    }
}

impl AccessControlStore {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_backend(Backend::Files(dir))
    }

    pub fn postgres(client: Arc<tokio_postgres::Client>) -> Self {
        Self::with_backend(Backend::Postgres(client))
    }

    fn with_backend(backend: Backend) -> Self {
        Self {
            backend,
            cache: Arc::new(tokio::sync::RwLock::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Connect to the deployment's PostgreSQL bot-config storage.
    pub async fn from_env() -> anyhow::Result<Self> {
        Ok(Self::postgres(postgres_client_from_env().await?))
    }

    pub async fn load(&self) -> AccessControl {
        if let Some((snapshot, fetched_at)) = self.cache.read().await.as_ref() {
            if fetched_at.elapsed() < ACCESS_CACHE_TTL {
                return snapshot.clone();
            }
        }
        match self.load_fresh().await {
            Ok(access) => {
                *self.cache.write().await = Some((access.clone(), Instant::now()));
                access
            }
            // Storage errors keep the last known policy instead of silently
            // falling open to permissive defaults.
            Err(_) => self
                .cache
                .read()
                .await
                .as_ref()
                .map(|(snapshot, _)| snapshot.clone())
                .unwrap_or_default(),
        }
    }

    async fn load_fresh(&self) -> anyhow::Result<AccessControl> {
        let bytes = self
            .backend
            .load(ACCESS_CONTROL_KEY, ACCESS_CONTROL_KEY)
            .await?;
        Ok(bytes
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default())
    }

    /// Atomically apply `mutate` to the freshly loaded state and persist the
    /// result. Fails without saving when the current state cannot be read.
    pub async fn update<T>(
        &self,
        mutate: impl FnOnce(&mut AccessControl) -> T,
    ) -> anyhow::Result<T> {
        let _guard = self.write_lock.lock().await;
        let mut access = self.load_fresh().await?;
        let outcome = mutate(&mut access);
        self.save(&access).await?;
        Ok(outcome)
    }

    pub async fn save(&self, access: &AccessControl) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(access)?;
        self.backend
            .save(ACCESS_CONTROL_KEY, ACCESS_CONTROL_KEY, data)
            .await?;
        *self.cache.write().await = Some((access.clone(), Instant::now()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labs_pagination_is_off_by_default() {
        assert!(!UserConfig::default().labs_pagination_enabled);
    }

    #[test]
    fn old_server_config_defaults_to_public_leaderboard() {
        let config: ServerConfig =
            serde_json::from_str(r#"{"allowed_channel_ids":[123]}"#).unwrap();
        assert_eq!(config.leaderboard_visibility, LeaderboardVisibility::Public);
        assert!(config.leaderboard_role_ids.is_empty());
        assert!(!config.respond_to_bot_pings);
        assert!(config.proactive_allowed);
        assert!(config.embed_enabled);
    }

    #[test]
    fn server_config_embed_enabled_persists_through_serde() {
        let mut config = ServerConfig::default();
        assert!(config.embed_enabled);
        config.embed_enabled = false;
        let json = serde_json::to_string(&config).unwrap();
        let restored: ServerConfig = serde_json::from_str(&json).unwrap();
        assert!(!restored.embed_enabled);
    }

    #[test]
    fn followup_is_off_by_default() {
        assert!(!UserConfig::default().followup_enabled);
    }

    #[test]
    fn old_user_config_defaults_labs_pagination_to_off() {
        let config: UserConfig =
            serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
        assert!(!config.labs_pagination_enabled);
        assert!(!config.followup_enabled);
        assert!(config.embed_enabled);
    }

    #[test]
    fn user_config_embed_enabled_persists_through_serde() {
        let config = UserConfig {
            embed_enabled: false,
            ..UserConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: UserConfig = serde_json::from_str(&json).unwrap();
        assert!(!restored.embed_enabled);
    }

    #[test]
    fn old_user_config_defaults_thinking_mode_to_medium() {
        let config: UserConfig =
            serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
        assert_eq!(config.thinking_mode, ThinkingMode::Medium);
    }

    #[test]
    fn old_user_config_defaults_progress_updates_to_enabled() {
        let config: UserConfig =
            serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
        assert!(config.progress_updates_enabled);
    }

    #[test]
    fn disabled_progress_updates_persist_through_serde() {
        let config = UserConfig {
            progress_updates_enabled: false,
            ..UserConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: UserConfig = serde_json::from_str(&json).unwrap();
        assert!(!restored.progress_updates_enabled);
    }

    #[test]
    fn thinking_mode_persists_through_serde() {
        let config = UserConfig {
            thinking_mode: ThinkingMode::XHigh,
            ..UserConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: UserConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.thinking_mode, ThinkingMode::XHigh);
    }

    #[test]
    fn deep_memory_is_on_by_default() {
        assert!(UserConfig::default().deep_memory_enabled);
    }

    #[test]
    fn proactive_assistance_is_off_by_default() {
        assert!(!UserConfig::default().proactive_assistance_enabled);
    }

    #[test]
    fn old_user_config_enables_memory_but_keeps_proactive_assistance_off() {
        let config: UserConfig =
            serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
        assert!(config.deep_memory_enabled);
        assert!(!config.proactive_assistance_enabled);
    }

    #[test]
    fn privacy_fields_persist_through_serde() {
        let config = UserConfig {
            deep_memory_enabled: true,
            proactive_assistance_enabled: true,
            ..UserConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: UserConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.deep_memory_enabled);
        assert!(restored.proactive_assistance_enabled);
    }

    #[test]
    fn owner_is_always_a_configurer_and_always_responded_to() {
        let mut access = AccessControl::default();
        access.user_policies.insert(
            42,
            UserPolicy {
                max_output_tokens: None,
                respond: false,
            },
        );
        assert!(access.is_configurer(42, 42));
        assert!(access.should_respond(42, 42));
        assert!(!access.is_configurer(42, 7));
        assert!(!access.should_respond(42, 7));
    }

    #[test]
    fn unset_owner_id_grants_no_access() {
        let access = AccessControl::default();
        assert!(!access.is_configurer(0, 0));
        assert!(access.should_respond(0, 0));
    }

    #[test]
    fn configurers_bypass_their_own_respond_policy() {
        let mut access = AccessControl::default();
        access.configurer_ids.insert(9);
        access.user_policies.insert(
            9,
            UserPolicy {
                max_output_tokens: Some(512),
                respond: false,
            },
        );
        assert!(access.is_configurer(9, 1));
        assert!(access.should_respond(9, 1));
        assert_eq!(access.policy(9).max_output_tokens, Some(512));
    }

    #[test]
    fn default_policy_responds_with_no_cap() {
        let access = AccessControl::default();
        let policy = access.policy(123);
        assert!(policy.respond);
        assert_eq!(policy.max_output_tokens, None);
    }

    #[test]
    fn access_control_round_trips_through_serde() {
        let mut access = AccessControl::default();
        access.configurer_ids.insert(11);
        access.user_policies.insert(
            22,
            UserPolicy {
                max_output_tokens: Some(2048),
                respond: false,
            },
        );
        let json = serde_json::to_string(&access).unwrap();
        let restored: AccessControl = serde_json::from_str(&json).unwrap();
        assert!(restored.configurer_ids.contains(&11));
        let policy = restored.policy(22);
        assert_eq!(policy.max_output_tokens, Some(2048));
        assert!(!policy.respond);
    }

    #[test]
    fn proactive_is_globally_enabled_by_default_and_for_old_configs() {
        assert!(AccessControl::default().proactive_enabled);
        let access: AccessControl = serde_json::from_str(r#"{"configurer_ids":[1]}"#).unwrap();
        assert!(access.proactive_enabled);
    }

    #[tokio::test]
    async fn access_store_round_trips_on_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = AccessControlStore::new(tmp.path().join("bot_config"));
        assert!(store.load().await.configurer_ids.is_empty());
        let mut access = AccessControl::default();
        access.configurer_ids.insert(5);
        store.save(&access).await.unwrap();
        assert!(store.load().await.configurer_ids.contains(&5));
    }

    #[tokio::test]
    async fn update_persists_mutations_and_reports_outcomes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = AccessControlStore::new(tmp.path().join("bot_config"));
        let inserted = store
            .update(|access| access.configurer_ids.insert(7))
            .await
            .unwrap();
        assert!(inserted);
        let inserted = store
            .update(|access| access.configurer_ids.insert(7))
            .await
            .unwrap();
        assert!(!inserted);
        assert!(store.load().await.configurer_ids.contains(&7));
    }
}
