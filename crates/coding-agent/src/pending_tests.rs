//! Unit tests for `pending` (split out to keep the module under 600 lines).

use super::*;

fn requester(user_id: u64, channel_id: u64, message_id: u64) -> DevelopmentRequester {
    DevelopmentRequester {
        user_id,
        username: "testuser".into(),
        channel_id,
        guild_id: None,
        source_message_id: message_id,
    }
}

fn source_msg(channel_id: u64, message_id: u64) -> DiscordMessageRef {
    DiscordMessageRef {
        channel_id,
        message_id,
    }
}

fn spec() -> DevelopmentSpecification {
    DevelopmentSpecification {
        title: "Test".into(),
        objective: "Do something".into(),
        context: String::new(),
        requirements: vec!["Req 1".into()],
        acceptance_criteria: vec!["AC 1".into()],
    }
}

fn make_job(
    owner_id: u64,
    requester_id: u64,
    channel_id: u64,
    message_id: u64,
    stage: DispatchStage,
) -> PendingDevelopmentJob {
    PendingDevelopmentJob::new(
        owner_id,
        requester(requester_id, channel_id, message_id),
        source_msg(channel_id, message_id),
        spec(),
        stage,
        PartialAgentSelection::default(),
    )
}

#[test]
fn non_owner_job_starts_in_awaiting_approval() {
    let job = make_job(1, 99, 2, 3, DispatchStage::AwaitingOwnerApproval);
    assert_eq!(job.stage, DispatchStage::AwaitingOwnerApproval);
    assert!(!job.is_expired());
}

#[test]
fn owner_interactive_job_starts_in_choosing_agent() {
    let job = make_job(1, 1, 2, 3, DispatchStage::ChoosingAgent);
    assert_eq!(job.stage, DispatchStage::ChoosingAgent);
}

#[test]
fn insert_and_retrieve() {
    let store = PendingJobStore::default();
    let job = make_job(1, 2, 2, 3, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    let found = store.with_job(id, |j| j.owner_id);
    assert_eq!(found, Some(1));
}

#[test]
fn with_job_mut_allows_stage_update() {
    let store = PendingJobStore::default();
    let job = make_job(1, 2, 2, 3, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    store.with_job_mut(id, |j| j.stage = DispatchStage::ChoosingModel);
    let stage = store.with_job(id, |j| j.stage);
    assert_eq!(stage, Some(DispatchStage::ChoosingModel));
}

#[test]
fn try_start_dispatch_only_succeeds_once() {
    let store = PendingJobStore::default();
    let job = make_job(1, 1, 2, 3, DispatchStage::ChoosingAgent);
    let id = store.insert(job);
    // Not in Confirming stage yet.
    assert!(!store.try_start_dispatch(id));
    // Advance to Confirming.
    store.with_job_mut(id, |j| j.stage = DispatchStage::Confirming);
    assert!(store.try_start_dispatch(id));
    // Second call returns false (already Dispatching).
    assert!(!store.try_start_dispatch(id));
}

#[test]
fn try_approve_with_defaults_requires_complete_selection() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 2, 3, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    // Selection is incomplete.
    assert!(!store.try_approve_with_defaults(id));
    // Fill selection.
    store.with_job_mut(id, |j| {
        j.selection.agent = Some(CodingAgent::Claude);
        j.selection.model = Some("model".into());
        j.selection.effort = Some("high".into());
    });
    assert!(store.try_approve_with_defaults(id));
    // Second call returns false.
    assert!(!store.try_approve_with_defaults(id));
}

#[test]
fn try_begin_configuration_from_awaiting() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 2, 3, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    assert!(store.try_begin_configuration(id));
    assert_eq!(
        store.with_job(id, |j| j.stage),
        Some(DispatchStage::ChoosingAgent)
    );
    // Cannot begin configuration again from ChoosingAgent.
    assert!(!store.try_begin_configuration(id));
}

#[test]
fn try_reject_succeeds_once_from_non_terminal() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 2, 3, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    assert!(store.try_reject(id));
    assert_eq!(
        store.with_job(id, |j| j.stage),
        Some(DispatchStage::Rejected)
    );
    // Already terminal — cannot reject again.
    assert!(!store.try_reject(id));
}

#[test]
fn rejected_job_cannot_dispatch() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 2, 3, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    store.try_reject(id);
    // Rejected is terminal, so try_start_dispatch should fail.
    assert!(!store.try_start_dispatch(id));
    assert!(!store.try_approve_with_defaults(id));
}

#[test]
fn mark_dispatched_changes_stage() {
    let store = PendingJobStore::default();
    let job = make_job(1, 1, 2, 3, DispatchStage::ChoosingAgent);
    let id = store.insert(job);
    store.with_job_mut(id, |j| j.stage = DispatchStage::Dispatching);
    store.mark_dispatched(id);
    assert_eq!(
        store.with_job(id, |j| j.stage),
        Some(DispatchStage::Dispatched)
    );
}

#[test]
fn mark_dispatch_failed_reverts_to_confirming() {
    let store = PendingJobStore::default();
    let job = make_job(1, 1, 2, 3, DispatchStage::ChoosingAgent);
    let id = store.insert(job);
    store.with_job_mut(id, |j| j.stage = DispatchStage::Dispatching);
    store.mark_dispatch_failed(id);
    assert_eq!(
        store.with_job(id, |j| j.stage),
        Some(DispatchStage::Confirming)
    );
}

#[test]
fn cancel_sets_stage() {
    let store = PendingJobStore::default();
    let job = make_job(1, 1, 2, 3, DispatchStage::ChoosingAgent);
    let id = store.insert(job);
    store.cancel(id);
    assert_eq!(
        store.with_job(id, |j| j.stage),
        Some(DispatchStage::Cancelled)
    );
}

#[test]
fn evict_expired_removes_non_dispatched_expired_jobs() {
    let store = PendingJobStore::default();
    let mut job = make_job(1, 2, 2, 3, DispatchStage::AwaitingOwnerApproval);
    job.expires_at = Utc::now() - chrono::Duration::seconds(1);
    let id = store.insert(job);
    store.evict_expired();
    assert!(store.with_job(id, |_| ()).is_none());
}

#[test]
fn evict_expired_keeps_dispatched_jobs() {
    let store = PendingJobStore::default();
    let mut job = make_job(1, 1, 2, 3, DispatchStage::ChoosingAgent);
    job.expires_at = Utc::now() - chrono::Duration::seconds(1);
    let id = store.insert(job);
    store.with_job_mut(id, |j| j.stage = DispatchStage::Dispatched);
    store.evict_expired();
    assert!(store.with_job(id, |_| ()).is_some());
}

#[test]
fn pending_for_requester_returns_active_jobs() {
    let store = PendingJobStore::default();
    let j1 = make_job(1, 99, 10, 100, DispatchStage::AwaitingOwnerApproval);
    let j2 = make_job(1, 99, 10, 101, DispatchStage::AwaitingOwnerApproval);
    let j3 = make_job(1, 55, 10, 102, DispatchStage::AwaitingOwnerApproval);
    let id1 = store.insert(j1);
    let id2 = store.insert(j2);
    store.insert(j3);
    let pending = store.pending_for_requester(99);
    assert_eq!(pending.len(), 2);
    assert!(pending.contains(&id1));
    assert!(pending.contains(&id2));
}

#[test]
fn pending_for_requester_excludes_terminal() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 10, 100, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    store.try_reject(id);
    assert!(store.pending_for_requester(99).is_empty());
}

#[test]
fn find_by_source_message() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 10, 100, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    assert_eq!(store.find_by_source_message(10, 100), Some(id));
    assert_eq!(store.find_by_source_message(10, 999), None);
}

#[test]
fn find_by_approval_message() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 10, 100, DispatchStage::AwaitingOwnerApproval);
    let id = store.insert(job);
    // Set approval message.
    store.with_job_mut(id, |j| {
        j.approval_message = Some(DiscordMessageRef {
            channel_id: 20,
            message_id: 200,
        });
    });
    assert_eq!(store.find_by_approval_message(20, 200), Some(id));
    assert_eq!(store.find_by_approval_message(10, 100), None);
}

#[test]
fn pending_count_excludes_terminal() {
    let store = PendingJobStore::default();
    let j1 = make_job(1, 99, 10, 100, DispatchStage::AwaitingOwnerApproval);
    let j2 = make_job(1, 88, 10, 101, DispatchStage::AwaitingOwnerApproval);
    let id1 = store.insert(j1);
    store.insert(j2);
    assert_eq!(store.pending_count(), 2);
    store.try_reject(id1);
    assert_eq!(store.pending_count(), 1);
}

#[test]
fn has_equivalent_pending_request_same_fingerprint() {
    let store = PendingJobStore::default();
    let job = make_job(1, 99, 10, 100, DispatchStage::AwaitingOwnerApproval);
    let fp = job.fingerprint.clone();
    store.insert(job);
    assert!(store.has_equivalent_pending_request(99, &fp));
    assert!(!store.has_equivalent_pending_request(88, &fp));
}

#[test]
fn spec_fingerprint_stable() {
    let s = spec();
    assert_eq!(s.fingerprint(), s.fingerprint());
}
