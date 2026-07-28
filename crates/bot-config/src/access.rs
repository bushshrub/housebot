//! Who may configure the bot, and the cached access-control store.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend::*;
use crate::user::default_respond;
use crate::*;
use anyhow::Context;
use housebot_config::data_dir;

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
    /// Channel the bot watches for the feature-development completion webhook
    /// (`/config dev_notify_channel`). `None` disables the watch.
    #[serde(default)]
    pub dev_notify_channel_id: Option<u64>,
}

impl Default for AccessControl {
    fn default() -> Self {
        Self {
            configurer_ids: HashSet::new(),
            user_policies: HashMap::new(),
            proactive_enabled: true,
            dev_notify_channel_id: None,
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

pub(crate) const ACCESS_CONTROL_KEY: &str = "access_control";

/// How long a cached access snapshot serves reads before storage is consulted
/// again. In-process saves refresh the cache immediately.
pub(crate) const ACCESS_CACHE_TTL: Duration = Duration::from_secs(30);

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
        match bytes {
            Some(bytes) => Ok(serde_json::from_slice(&bytes)
                .context("stored access control snapshot is not valid JSON")?),
            None => Ok(AccessControl::default()),
        }
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
