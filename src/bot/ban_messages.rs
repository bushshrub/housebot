//! Rendering for ban/restore proposal, approval, and rejection notices.

pub(super) fn format_proposal_message(
    proposal: &crate::tool_permissions::BanProposal,
    approvals: usize,
    rejections: usize,
    min_votes: usize,
) -> String {
    format!(
        "🗳️ **Ban proposal** by <@{}>\n\
         Target: <@{}>\n\
         Tool: `{}`\n\
         Votes: **{} approve** / **{} reject** (minimum {} votes)\n\
         React with ✅ to approve, ❌ to reject (or use `/tool_ban vote`)\n\
         Proposal ID: `{}`",
        proposal.proposed_by,
        proposal.target_user_id,
        proposal.tool_name,
        approvals,
        rejections,
        min_votes,
        proposal.id.get(..8).unwrap_or(&proposal.id),
    )
}

pub(super) fn format_approved_message(ban: &crate::tool_permissions::ToolBan) -> String {
    format!(
        "🚫 **Ban approved!** <@{}> is now blocked from using `{}`.",
        ban.user_id, ban.tool_name
    )
}

pub(super) fn format_rejected_message(proposal: &crate::tool_permissions::BanProposal) -> String {
    format!(
        "❌ **Ban rejected.** The proposal to restrict <@{}> from `{}` did not pass.",
        proposal.target_user_id, proposal.tool_name
    )
}

// ── Restore proposal message formatting helpers ──────────────────────────────

pub(super) fn format_restore_proposal_message(
    proposal: &crate::tool_permissions::UnbanProposal,
    approvals: usize,
    rejections: usize,
    min_votes: usize,
) -> String {
    format!(
        "🔓 **Restore proposal** by <@{}>\n\
         Target: <@{}>\n\
         Tool: `{}`\n\
         Votes: **{} approve** / **{} reject** (minimum {} votes)\n\
         React with ✅ to approve restore, ❌ to reject (or use `/tool_restore vote`)\n\
         Proposal ID: `{}`",
        proposal.proposed_by,
        proposal.target_user_id,
        proposal.tool_name,
        approvals,
        rejections,
        min_votes,
        proposal.id.get(..8).unwrap_or(&proposal.id),
    )
}

pub(super) fn format_restore_approved_message(ban: &crate::tool_permissions::ToolBan) -> String {
    format!(
        "✅ **Restore approved!** <@{}>'s access to `{}` has been restored.",
        ban.user_id, ban.tool_name
    )
}

pub(super) fn format_restore_rejected_message(
    proposal: &crate::tool_permissions::UnbanProposal,
) -> String {
    format!(
        "❌ **Restore rejected.** The proposal to restore <@{}>'s access to `{}` did not pass.",
        proposal.target_user_id, proposal.tool_name
    )
}
