//! Guild-scoped tool bans decided by member voting.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config;
use crate::memory::ensure_dir;

const PROPOSAL_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolBan {
    pub guild_id: u64,
    pub user_id: u64,
    pub tool_name: String,
    pub approved_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BanProposal {
    pub id: String,
    pub guild_id: u64,
    pub target_user_id: u64,
    pub tool_name: String,
    pub proposed_by: u64,
    pub created_at: u64,
    pub expires_at: u64,
    pub votes: HashMap<u64, bool>,
    #[serde(default)]
    pub channel_id: u64,
    #[serde(default)]
    pub message_id: u64,
}

impl BanProposal {
    pub fn vote_counts(&self) -> (usize, usize) {
        let approvals = self.votes.values().filter(|vote| **vote).count();
        (approvals, self.votes.len().saturating_sub(approvals))
    }
}

/// A proposal to lift an existing tool ban via member voting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnbanProposal {
    pub id: String,
    pub guild_id: u64,
    pub target_user_id: u64,
    pub tool_name: String,
    pub proposed_by: u64,
    pub created_at: u64,
    pub expires_at: u64,
    pub votes: HashMap<u64, bool>,
    #[serde(default)]
    pub channel_id: u64,
    #[serde(default)]
    pub message_id: u64,
}

impl UnbanProposal {
    pub fn vote_counts(&self) -> (usize, usize) {
        let approvals = self.votes.values().filter(|vote| **vote).count();
        (approvals, self.votes.len().saturating_sub(approvals))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PermissionState {
    #[serde(default)]
    bans: Vec<ToolBan>,
    #[serde(default)]
    proposals: Vec<BanProposal>,
    #[serde(default)]
    restore_proposals: Vec<UnbanProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoteResult {
    Pending {
        approvals: usize,
        rejections: usize,
        quorum: usize,
    },
    Approved(ToolBan),
    Rejected,
    /// A ban was lifted by a successful restore vote.
    RestoreVoted(ToolBan),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuildPermissionStatus {
    pub bans: Vec<ToolBan>,
    pub proposals: Vec<BanProposal>,
    pub restore_proposals: Vec<UnbanProposal>,
}

/// Persistent, concurrency-safe permission store.
#[derive(Clone)]
pub struct ToolPermissions {
    path: PathBuf,
    min_votes: usize,
    lock: Arc<Mutex<()>>,
}

impl Default for ToolPermissions {
    fn default() -> Self {
        Self::new(
            config::data_dir().join("tool_permissions.json"),
            config::env_parse("TOOL_BAN_MIN_VOTES", 3),
        )
    }
}

impl ToolPermissions {
    pub fn new(path: impl Into<PathBuf>, min_votes: usize) -> Self {
        Self {
            path: path.into(),
            min_votes: min_votes.max(2),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn min_votes(&self) -> usize {
        self.min_votes
    }

    pub async fn is_banned(
        &self,
        guild_id: u64,
        user_id: u64,
        tool_name: &str,
    ) -> std::io::Result<bool> {
        let _guard = self.lock.lock().await;
        let state = self.load().await?;
        let tool_name = tool_name.to_ascii_lowercase();
        Ok(state.bans.iter().any(|ban| {
            ban.guild_id == guild_id
                && ban.user_id == user_id
                && ban.tool_name.eq_ignore_ascii_case(&tool_name)
        }))
    }

    pub async fn propose(
        &self,
        guild_id: u64,
        target_user_id: u64,
        tool_name: &str,
        proposed_by: u64,
    ) -> Result<BanProposal, String> {
        if guild_id == 0 {
            return Err("Tool-ban voting is only available inside a server.".into());
        }
        if target_user_id == proposed_by {
            return Err("You cannot propose a tool ban against yourself.".into());
        }
        let tool_name = validate_tool_name(tool_name)?;
        let _guard = self.lock.lock().await;
        let now = unix_now();
        let mut state = self.load().await.map_err(|error| error.to_string())?;
        state.proposals.retain(|proposal| proposal.expires_at > now);
        if state.bans.iter().any(|ban| {
            ban.guild_id == guild_id && ban.user_id == target_user_id && ban.tool_name == tool_name
        }) {
            return Err(format!(
                "That user is already banned from `{tool_name}` in this server."
            ));
        }
        if state.proposals.iter().any(|proposal| {
            proposal.guild_id == guild_id
                && proposal.target_user_id == target_user_id
                && proposal.tool_name == tool_name
        }) {
            return Err("An open proposal already covers that user and tool.".into());
        }
        let mut votes = HashMap::new();
        votes.insert(proposed_by, true);
        let proposal = BanProposal {
            id: uuid::Uuid::new_v4().simple().to_string(),
            guild_id,
            target_user_id,
            tool_name,
            proposed_by,
            created_at: now,
            expires_at: now.saturating_add(PROPOSAL_TTL_SECS),
            votes,
            channel_id: 0,
            message_id: 0,
        };
        state.proposals.push(proposal.clone());
        self.save(&state).await.map_err(|error| error.to_string())?;
        Ok(proposal)
    }

    pub async fn vote(
        &self,
        guild_id: u64,
        proposal_id: &str,
        voter_id: u64,
        approve: bool,
    ) -> Result<VoteResult, String> {
        if proposal_id.trim().len() < 4 {
            return Err("Provide at least four characters of the proposal ID.".into());
        }
        let _guard = self.lock.lock().await;
        let now = unix_now();
        let mut state = self.load().await.map_err(|error| error.to_string())?;
        state.proposals.retain(|proposal| proposal.expires_at > now);
        let Some(index) = state.proposals.iter().position(|proposal| {
            proposal.guild_id == guild_id && proposal.id.starts_with(proposal_id)
        }) else {
            return Err("No open proposal matches that ID in this server.".into());
        };
        if state
            .proposals
            .iter()
            .filter(|proposal| {
                proposal.guild_id == guild_id && proposal.id.starts_with(proposal_id)
            })
            .count()
            > 1
        {
            return Err("That proposal ID prefix is ambiguous; provide more characters.".into());
        }
        if state.proposals[index].target_user_id == voter_id {
            return Err("The targeted user cannot vote on their own restriction.".into());
        }
        state.proposals[index].votes.insert(voter_id, approve);
        let (approvals, rejections) = state.proposals[index].vote_counts();
        let total = approvals + rejections;
        let result = if total >= self.min_votes && approvals > rejections {
            let proposal = state.proposals.remove(index);
            let ban = ToolBan {
                guild_id: proposal.guild_id,
                user_id: proposal.target_user_id,
                tool_name: proposal.tool_name,
                approved_at: now,
            };
            state.bans.push(ban.clone());
            VoteResult::Approved(ban)
        } else if total >= self.min_votes && rejections > approvals {
            state.proposals.remove(index);
            VoteResult::Rejected
        } else {
            VoteResult::Pending {
                approvals,
                rejections,
                quorum: self.min_votes,
            }
        };
        self.save(&state).await.map_err(|error| error.to_string())?;
        Ok(result)
    }

    /// Attach a Discord channel + message ID to an existing proposal (for emoji voting).
    pub async fn set_proposal_message(
        &self,
        guild_id: u64,
        proposal_id: &str,
        channel_id: u64,
        message_id: u64,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        let mut state = self.load().await.map_err(|e| e.to_string())?;
        let Some(proposal) = state
            .proposals
            .iter_mut()
            .find(|p| p.guild_id == guild_id && p.id == proposal_id)
        else {
            return Err("Proposal not found.".into());
        };
        proposal.channel_id = channel_id;
        proposal.message_id = message_id;
        self.save(&state).await.map_err(|e| e.to_string())
    }

    /// Remove a proposal (used for rollback on publication failure).
    pub async fn remove_proposal(&self, guild_id: u64, proposal_id: &str) -> std::io::Result<()> {
        let _guard = self.lock.lock().await;
        let mut state = self.load().await?;
        state
            .proposals
            .retain(|p| p.guild_id != guild_id || p.id != proposal_id);
        self.save(&state).await
    }

    /// Look up a proposal by its Discord message ID.
    pub async fn find_by_message(
        &self,
        message_id: u64,
    ) -> std::io::Result<Option<(String, BanProposal)>> {
        let _guard = self.lock.lock().await;
        let state = self.load().await?;
        for proposal in &state.proposals {
            if proposal.message_id == message_id {
                return Ok(Some((proposal.id.clone(), proposal.clone())));
            }
        }
        Ok(None)
    }

    /// Look up a proposal by its ID prefix (for slash-command votes that need
    /// channel/message IDs before calling vote).
    pub async fn find_proposal_by_prefix(
        &self,
        guild_id: u64,
        prefix: &str,
    ) -> std::io::Result<Option<BanProposal>> {
        let _guard = self.lock.lock().await;
        let state = self.load().await?;
        let now = unix_now();
        Ok(state
            .proposals
            .iter()
            .find(|p| p.guild_id == guild_id && p.id.starts_with(prefix) && p.expires_at > now)
            .cloned())
    }

    pub async fn status(&self, guild_id: u64) -> std::io::Result<GuildPermissionStatus> {
        let _guard = self.lock.lock().await;
        let now = unix_now();
        let state = self.load().await?;
        Ok(GuildPermissionStatus {
            bans: state
                .bans
                .into_iter()
                .filter(|ban| ban.guild_id == guild_id)
                .collect(),
            proposals: state
                .proposals
                .into_iter()
                .filter(|proposal| proposal.guild_id == guild_id && proposal.expires_at > now)
                .collect(),
            restore_proposals: state
                .restore_proposals
                .into_iter()
                .filter(|p| p.guild_id == guild_id && p.expires_at > now)
                .collect(),
        })
    }

    /// Propose restoring tool access that was previously banned.
    pub async fn propose_restore(
        &self,
        guild_id: u64,
        target_user_id: u64,
        tool_name: &str,
        proposed_by: u64,
    ) -> Result<UnbanProposal, String> {
        if guild_id == 0 {
            return Err("Tool-restore voting is only available inside a server.".into());
        }
        let tool_name = validate_tool_name(tool_name)?;
        let _guard = self.lock.lock().await;
        let now = unix_now();
        let mut state = self.load().await.map_err(|error| error.to_string())?;
        state.restore_proposals.retain(|p| p.expires_at > now);
        let has_ban = state.bans.iter().any(|ban| {
            ban.guild_id == guild_id && ban.user_id == target_user_id && ban.tool_name == tool_name
        });
        if !has_ban {
            return Err(format!(
                "That user is not currently banned from `{tool_name}` in this server."
            ));
        }
        if state.restore_proposals.iter().any(|p| {
            p.guild_id == guild_id && p.target_user_id == target_user_id && p.tool_name == tool_name
        }) {
            return Err("An open restore proposal already covers that user and tool.".into());
        }
        let mut votes = HashMap::new();
        votes.insert(proposed_by, true);
        let proposal = UnbanProposal {
            id: uuid::Uuid::new_v4().simple().to_string(),
            guild_id,
            target_user_id,
            tool_name,
            proposed_by,
            created_at: now,
            expires_at: now.saturating_add(PROPOSAL_TTL_SECS),
            votes,
            channel_id: 0,
            message_id: 0,
        };
        state.restore_proposals.push(proposal.clone());
        self.save(&state).await.map_err(|error| error.to_string())?;
        Ok(proposal)
    }

    /// Vote on a tool-restore proposal.
    pub async fn vote_restore(
        &self,
        guild_id: u64,
        proposal_id: &str,
        voter_id: u64,
        approve: bool,
    ) -> Result<VoteResult, String> {
        if proposal_id.trim().len() < 4 {
            return Err("Provide at least four characters of the proposal ID.".into());
        }
        let _guard = self.lock.lock().await;
        let now = unix_now();
        let mut state = self.load().await.map_err(|error| error.to_string())?;
        state.restore_proposals.retain(|p| p.expires_at > now);
        let Some(index) = state
            .restore_proposals
            .iter()
            .position(|p| p.guild_id == guild_id && p.id.starts_with(proposal_id))
        else {
            return Err("No open restore proposal matches that ID in this server.".into());
        };
        if state
            .restore_proposals
            .iter()
            .filter(|p| p.guild_id == guild_id && p.id.starts_with(proposal_id))
            .count()
            > 1
        {
            return Err("That proposal ID prefix is ambiguous; provide more characters.".into());
        }
        state.restore_proposals[index]
            .votes
            .insert(voter_id, approve);
        let (approvals, rejections) = state.restore_proposals[index].vote_counts();
        let total = approvals + rejections;
        let result = if total >= self.min_votes && approvals > rejections {
            let proposal = state.restore_proposals.remove(index);
            let tool_name = proposal.tool_name;
            let removed_ban_idx = state.bans.iter().position(|ban| {
                ban.guild_id == guild_id
                    && ban.user_id == proposal.target_user_id
                    && ban.tool_name == tool_name
            });
            match removed_ban_idx {
                Some(idx) => {
                    let ban = state.bans.remove(idx);
                    VoteResult::RestoreVoted(ban)
                }
                None => VoteResult::Rejected,
            }
        } else if total >= self.min_votes && rejections > approvals {
            state.restore_proposals.remove(index);
            VoteResult::Rejected
        } else {
            VoteResult::Pending {
                approvals,
                rejections,
                quorum: self.min_votes,
            }
        };
        self.save(&state).await.map_err(|error| error.to_string())?;
        Ok(result)
    }

    /// Attach channel + message IDs to a restore proposal (for emoji voting).
    pub async fn set_restore_proposal_message(
        &self,
        guild_id: u64,
        proposal_id: &str,
        channel_id: u64,
        message_id: u64,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        let mut state = self.load().await.map_err(|e| e.to_string())?;
        let Some(p) = state
            .restore_proposals
            .iter_mut()
            .find(|p| p.guild_id == guild_id && p.id == proposal_id)
        else {
            return Err("Restore proposal not found.".into());
        };
        p.channel_id = channel_id;
        p.message_id = message_id;
        self.save(&state).await.map_err(|e| e.to_string())
    }

    /// Remove a restore proposal (used for rollback on publication failure).
    pub async fn remove_restore_proposal(
        &self,
        guild_id: u64,
        proposal_id: &str,
    ) -> std::io::Result<()> {
        let _guard = self.lock.lock().await;
        let mut state = self.load().await?;
        state
            .restore_proposals
            .retain(|p| p.guild_id != guild_id || p.id != proposal_id);
        self.save(&state).await
    }

    /// Look up a restore proposal by its Discord message ID.
    pub async fn find_restore_by_message(
        &self,
        message_id: u64,
    ) -> std::io::Result<Option<(String, UnbanProposal)>> {
        let _guard = self.lock.lock().await;
        let state = self.load().await?;
        for p in &state.restore_proposals {
            if p.message_id == message_id {
                return Ok(Some((p.id.clone(), p.clone())));
            }
        }
        Ok(None)
    }

    /// Look up a restore proposal by its ID prefix.
    pub async fn find_restore_proposal_by_prefix(
        &self,
        guild_id: u64,
        prefix: &str,
    ) -> std::io::Result<Option<UnbanProposal>> {
        let _guard = self.lock.lock().await;
        let state = self.load().await?;
        let now = unix_now();
        Ok(state
            .restore_proposals
            .iter()
            .find(|p| p.guild_id == guild_id && p.id.starts_with(prefix) && p.expires_at > now)
            .cloned())
    }

    async fn load(&self) -> std::io::Result<PermissionState> {
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PermissionState::default());
            }
            Err(error) => return Err(error),
        };
        serde_json::from_str(&raw).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "failed to parse tool permissions at {}: {error}",
                    self.path.display()
                ),
            )
        })
    }

    async fn save(&self, state: &PermissionState) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent).await?;
        }
        let body = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        tokio::fs::write(&temporary, body).await?;
        tokio::fs::rename(temporary, &self.path).await
    }
}

fn validate_tool_name(tool_name: &str) -> Result<String, String> {
    let tool_name = tool_name.trim().to_ascii_lowercase();
    if tool_name.is_empty() || tool_name.len() > 128 {
        return Err("Tool names must contain between 1 and 128 characters.".into());
    }
    if !tool_name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(
            "Tool names may contain only letters, numbers, underscores, and hyphens.".into(),
        );
    }
    Ok(tool_name)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store(min_votes: usize) -> (TempDir, ToolPermissions) {
        let temp = TempDir::new().unwrap();
        let store = ToolPermissions::new(temp.path().join("permissions.json"), min_votes);
        (temp, store)
    }

    #[tokio::test]
    async fn majority_approval_creates_enforced_ban() {
        let (_temp, store) = store(3);
        let proposal = store.propose(10, 200, "web_search", 100).await.unwrap();
        assert!(matches!(
            store.vote(10, &proposal.id, 101, true).await.unwrap(),
            VoteResult::Pending { .. }
        ));
        assert!(matches!(
            store.vote(10, &proposal.id, 102, false).await.unwrap(),
            VoteResult::Approved(_)
        ));
        assert!(store.is_banned(10, 200, "web_search").await.unwrap());
        assert!(!store.is_banned(11, 200, "web_search").await.unwrap());
    }

    #[tokio::test]
    async fn majority_rejection_closes_proposal_without_ban() {
        let (_temp, store) = store(3);
        let proposal = store.propose(10, 200, "translate", 100).await.unwrap();
        store.vote(10, &proposal.id, 101, false).await.unwrap();
        assert_eq!(
            store.vote(10, &proposal.id, 102, false).await.unwrap(),
            VoteResult::Rejected
        );
        assert!(!store.is_banned(10, 200, "translate").await.unwrap());
        assert!(store.status(10).await.unwrap().proposals.is_empty());
    }

    #[tokio::test]
    async fn target_cannot_vote_and_voters_can_change_vote() {
        let (_temp, store) = store(4);
        let proposal = store.propose(10, 200, "translate", 100).await.unwrap();
        assert!(store.vote(10, &proposal.id, 200, true).await.is_err());
        store.vote(10, &proposal.id, 101, false).await.unwrap();
        let result = store.vote(10, &proposal.id, 101, true).await.unwrap();
        assert_eq!(
            result,
            VoteResult::Pending {
                approvals: 2,
                rejections: 0,
                quorum: 4
            }
        );
    }

    #[tokio::test]
    async fn prevents_duplicate_and_self_targeted_proposals() {
        let (_temp, store) = store(3);
        assert!(store.propose(10, 100, "web_search", 100).await.is_err());
        store.propose(10, 200, "web_search", 100).await.unwrap();
        assert!(store.propose(10, 200, "web_search", 101).await.is_err());
    }

    #[tokio::test]
    async fn corrupt_state_fails_closed_instead_of_dropping_bans() {
        let (temp, store) = store(3);
        tokio::fs::write(temp.path().join("permissions.json"), "not-json")
            .await
            .unwrap();
        assert!(store.is_banned(10, 200, "web_search").await.is_err());
        assert!(store.propose(10, 200, "web_search", 100).await.is_err());
    }

    // ── restore voting tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn restore_proposal_fails_without_existing_ban() {
        let (_temp, store) = store(3);
        let err = store
            .propose_restore(10, 200, "web_search", 100)
            .await
            .unwrap_err();
        assert!(err.contains("not currently banned"));
    }

    #[tokio::test]
    async fn restore_approval_removes_ban() {
        let (_temp, store) = store(3);
        // First create a ban
        let ban_proposal = store.propose(10, 200, "web_search", 100).await.unwrap();
        store.vote(10, &ban_proposal.id, 101, true).await.unwrap();
        store.vote(10, &ban_proposal.id, 102, false).await.unwrap();
        assert!(store.is_banned(10, 200, "web_search").await.unwrap());

        // Now propose to restore
        let restore = store
            .propose_restore(10, 200, "web_search", 300)
            .await
            .unwrap();
        // Vote to approve the restore
        store
            .vote_restore(10, &restore.id, 101, true)
            .await
            .unwrap();
        let result = store
            .vote_restore(10, &restore.id, 102, true)
            .await
            .unwrap();
        assert!(matches!(result, VoteResult::RestoreVoted(_)));
        // Ban should be gone
        assert!(!store.is_banned(10, 200, "web_search").await.unwrap());
    }

    #[tokio::test]
    async fn restore_rejection_keeps_ban() {
        let (_temp, store) = store(3);
        let ban_proposal = store.propose(10, 200, "translate", 100).await.unwrap();
        store.vote(10, &ban_proposal.id, 101, true).await.unwrap();
        store.vote(10, &ban_proposal.id, 102, false).await.unwrap();

        let restore = store
            .propose_restore(10, 200, "translate", 300)
            .await
            .unwrap();
        store
            .vote_restore(10, &restore.id, 101, false)
            .await
            .unwrap();
        let result = store
            .vote_restore(10, &restore.id, 102, false)
            .await
            .unwrap();
        assert_eq!(result, VoteResult::Rejected);
        // Ban should still be in place
        assert!(store.is_banned(10, 200, "translate").await.unwrap());
    }

    #[tokio::test]
    async fn targeted_user_can_vote_on_own_restoration() {
        let (_temp, store) = store(3);
        let ban_proposal = store.propose(10, 200, "web_search", 100).await.unwrap();
        store.vote(10, &ban_proposal.id, 101, true).await.unwrap();
        store.vote(10, &ban_proposal.id, 102, false).await.unwrap();

        let restore = store
            .propose_restore(10, 200, "web_search", 300)
            .await
            .unwrap();
        // Target (200) can vote on their own restoration
        assert!(store.vote_restore(10, &restore.id, 200, true).await.is_ok());
    }

    #[tokio::test]
    async fn prevents_duplicate_restore_proposals() {
        let (_temp, store) = store(3);
        let ban_proposal = store.propose(10, 200, "web_search", 100).await.unwrap();
        store.vote(10, &ban_proposal.id, 101, true).await.unwrap();
        store.vote(10, &ban_proposal.id, 102, false).await.unwrap();

        store
            .propose_restore(10, 200, "web_search", 300)
            .await
            .unwrap();
        assert!(store
            .propose_restore(10, 200, "web_search", 301)
            .await
            .is_err());
    }
}
