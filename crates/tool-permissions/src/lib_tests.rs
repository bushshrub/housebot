//! Unit tests for `lib` (split out to keep the module under 400 lines).

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
