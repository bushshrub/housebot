//! Unban ("restore") proposals and voting.
use crate::*;

impl ToolPermissions {
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
}
