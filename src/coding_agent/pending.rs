//! Pending coding-job state machine.
//!
//! A `PendingDevelopmentJob` is created when the LLM calls `prepare_feature_development`
//! and destroyed when it is dispatched, cancelled, or expires.
//! Jobs are held in memory only; no persistence across restarts (acceptable because no
//! remote job starts before the owner confirms dispatch).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::catalog::CodingAgent;

/// Default expiry for owner-interactive and non-owner-approval jobs.
const DEFAULT_EXPIRY_SECS: u64 = 3600; // 1 hour, configurable via DEVELOPMENT_APPROVAL_EXPIRY_SECS

fn expiry_duration() -> Duration {
    let secs: u64 =
        crate::config::env_parse("DEVELOPMENT_APPROVAL_EXPIRY_SECS", DEFAULT_EXPIRY_SECS);
    Duration::from_secs(secs)
}

/// The structured specification built from the LLM tool call.
#[derive(Debug, Clone)]
pub struct DevelopmentSpecification {
    pub title: String,
    pub objective: String,
    pub context: String,
    pub requirements: Vec<String>,
    pub acceptance_criteria: Vec<String>,
}

impl DevelopmentSpecification {
    /// Deterministic fingerprint for duplicate-suppression.
    pub fn fingerprint(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.title.trim().to_lowercase().as_bytes());
        h.update(b"\x00");
        h.update(self.objective.trim().to_lowercase().as_bytes());
        h.update(b"\x00");
        let mut reqs: Vec<String> = self
            .requirements
            .iter()
            .map(|r| r.trim().to_lowercase())
            .collect();
        reqs.sort();
        h.update(reqs.join("\x01").as_bytes());
        h.update(b"\x00");
        let mut acs: Vec<String> = self
            .acceptance_criteria
            .iter()
            .map(|a| a.trim().to_lowercase())
            .collect();
        acs.sort();
        h.update(acs.join("\x01").as_bytes());
        format!("{:x}", h.finalize())
    }
}

/// Partially-filled agent/model/effort selection built during the Discord component flow.
#[derive(Debug, Clone, Default)]
pub struct PartialAgentSelection {
    pub agent: Option<CodingAgent>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl PartialAgentSelection {
    pub fn is_complete(&self) -> bool {
        self.agent.is_some() && self.model.is_some() && self.effort.is_some()
    }
}

/// Stages of the multi-step flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStage {
    /// Non-owner request: waiting for owner to approve, configure, or reject.
    AwaitingOwnerApproval,
    ChoosingAgent,
    ChoosingModel,
    ChoosingEffort,
    Confirming,
    /// Atomic transition: issue is being created.
    Dispatching,
    /// Issue created successfully.
    Dispatched,
    Rejected,
    Cancelled,
}

impl DispatchStage {
    /// True if the job is in a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Dispatched | Self::Rejected | Self::Cancelled)
    }
}

/// Identity of the user who originally submitted the development request.
#[derive(Debug, Clone)]
pub struct DevelopmentRequester {
    pub user_id: u64,
    pub username: String,
    pub channel_id: u64,
    pub guild_id: Option<u64>,
    pub source_message_id: u64,
}

/// A stable reference to a Discord message.
#[derive(Debug, Clone, Copy)]
pub struct DiscordMessageRef {
    pub channel_id: u64,
    pub message_id: u64,
}

/// A pending coding job.
pub struct PendingDevelopmentJob {
    pub id: Uuid,
    pub owner_id: u64,
    pub requester: DevelopmentRequester,
    /// The approval DM or channel message sent to the owner (set after send).
    pub approval_message: Option<DiscordMessageRef>,
    /// The original source Discord message reference.
    pub source_message: DiscordMessageRef,
    pub specification: DevelopmentSpecification,
    pub selection: PartialAgentSelection,
    pub stage: DispatchStage,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub fingerprint: String,
}

impl PendingDevelopmentJob {
    pub fn new(
        owner_id: u64,
        requester: DevelopmentRequester,
        source_message: DiscordMessageRef,
        spec: DevelopmentSpecification,
        initial_stage: DispatchStage,
        selection: PartialAgentSelection,
    ) -> Self {
        let now = Utc::now();
        let dur = expiry_duration();
        let expires_at = now
            + chrono::Duration::from_std(dur).expect("expiry duration fits in chrono::Duration");
        let fingerprint = spec.fingerprint();
        Self {
            id: Uuid::new_v4(),
            owner_id,
            requester,
            approval_message: None,
            source_message,
            specification: spec,
            selection,
            stage: initial_stage,
            created_at: now,
            expires_at,
            fingerprint,
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// True if this job is still awaiting action (not terminal, not dispatching).
    pub fn is_active(&self) -> bool {
        !self.stage.is_terminal() && self.stage != DispatchStage::Dispatching
    }
}

/// In-memory store for pending jobs, shared between `Agent` and `HouseBot`.
pub struct PendingJobStore {
    jobs: Mutex<HashMap<Uuid, PendingDevelopmentJob>>,
}

impl Default for PendingJobStore {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

impl PendingJobStore {
    pub fn insert(&self, job: PendingDevelopmentJob) -> Uuid {
        let id = job.id;
        self.jobs.lock().unwrap().insert(id, job);
        id
    }

    pub fn with_job<F, T>(&self, id: Uuid, f: F) -> Option<T>
    where
        F: FnOnce(&PendingDevelopmentJob) -> T,
    {
        let jobs = self.jobs.lock().unwrap();
        jobs.get(&id).map(f)
    }

    pub fn with_job_mut<F, T>(&self, id: Uuid, f: F) -> Option<T>
    where
        F: FnOnce(&mut PendingDevelopmentJob) -> T,
    {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.get_mut(&id).map(f)
    }

    /// Atomically transition `AwaitingOwnerApproval` → `Dispatching` when selection is complete.
    ///
    /// Returns `true` exactly once per job.
    pub fn try_approve_with_defaults(&self, id: Uuid) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            if job.stage == DispatchStage::AwaitingOwnerApproval
                && !job.is_expired()
                && job.selection.is_complete()
            {
                job.stage = DispatchStage::Dispatching;
                return true;
            }
        }
        false
    }

    /// Atomically transition `AwaitingOwnerApproval` → `ChoosingAgent` for interactive config.
    pub fn try_begin_configuration(&self, id: Uuid) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            if job.stage == DispatchStage::AwaitingOwnerApproval && !job.is_expired() {
                job.stage = DispatchStage::ChoosingAgent;
                return true;
            }
        }
        false
    }

    /// Atomically transition any non-terminal, non-dispatching job → `Rejected`.
    pub fn try_reject(&self, id: Uuid) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            if !job.stage.is_terminal() && job.stage != DispatchStage::Dispatching {
                job.stage = DispatchStage::Rejected;
                return true;
            }
        }
        false
    }

    /// Atomically transition a `Confirming` job → `Dispatching`.
    ///
    /// Returns `true` exactly once per job (prevents double-dispatch).
    pub fn try_start_dispatch(&self, id: Uuid) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            if job.stage == DispatchStage::Confirming && !job.is_expired() {
                job.stage = DispatchStage::Dispatching;
                return true;
            }
        }
        false
    }

    pub fn mark_dispatched(&self, id: Uuid) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            job.stage = DispatchStage::Dispatched;
        }
    }

    pub fn mark_dispatch_failed(&self, id: Uuid) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            job.stage = match job.stage {
                DispatchStage::Dispatching => DispatchStage::Confirming,
                other => other,
            };
        }
    }

    pub fn cancel(&self, id: Uuid) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            job.stage = DispatchStage::Cancelled;
        }
    }

    /// All non-terminal job IDs for a given requester.
    pub fn pending_for_requester(&self, requester_id: u64) -> Vec<Uuid> {
        let jobs = self.jobs.lock().unwrap();
        jobs.values()
            .filter(|j| j.requester.user_id == requester_id && !j.stage.is_terminal())
            .map(|j| j.id)
            .collect()
    }

    /// Find a job whose source message matches the given channel + message ID.
    pub fn find_by_source_message(&self, channel_id: u64, message_id: u64) -> Option<Uuid> {
        let jobs = self.jobs.lock().unwrap();
        jobs.values()
            .find(|j| {
                j.source_message.channel_id == channel_id
                    && j.source_message.message_id == message_id
                    && !j.stage.is_terminal()
            })
            .map(|j| j.id)
    }

    /// Find a job whose approval message matches the given channel + message ID.
    pub fn find_by_approval_message(&self, channel_id: u64, message_id: u64) -> Option<Uuid> {
        let jobs = self.jobs.lock().unwrap();
        jobs.values()
            .find(|j| {
                j.approval_message
                    .map(|r| r.channel_id == channel_id && r.message_id == message_id)
                    .unwrap_or(false)
                    && !j.stage.is_terminal()
            })
            .map(|j| j.id)
    }

    /// Number of non-terminal jobs in the store.
    pub fn pending_count(&self) -> usize {
        let jobs = self.jobs.lock().unwrap();
        jobs.values().filter(|j| !j.stage.is_terminal()).count()
    }

    /// Number of jobs in `AwaitingOwnerApproval`.
    pub fn awaiting_approval_count(&self) -> usize {
        let jobs = self.jobs.lock().unwrap();
        jobs.values()
            .filter(|j| j.stage == DispatchStage::AwaitingOwnerApproval)
            .count()
    }

    /// True if the same requester already has a pending request with this fingerprint.
    pub fn has_equivalent_pending_request(&self, requester_id: u64, fingerprint: &str) -> bool {
        let jobs = self.jobs.lock().unwrap();
        jobs.values().any(|j| {
            j.requester.user_id == requester_id
                && j.fingerprint == fingerprint
                && !j.stage.is_terminal()
        })
    }

    /// Find the single awaiting-approval job in a given DM channel (for text approval).
    pub fn find_awaiting_in_channel(&self, channel_id: u64) -> Option<Uuid> {
        let jobs = self.jobs.lock().unwrap();
        let matches: Vec<Uuid> = jobs
            .values()
            .filter(|j| {
                j.stage == DispatchStage::AwaitingOwnerApproval
                    && j.approval_message
                        .map(|r| r.channel_id == channel_id)
                        .unwrap_or(false)
            })
            .map(|j| j.id)
            .collect();
        if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        }
    }

    /// Remove jobs that expired and were never dispatched.
    pub fn evict_expired(&self) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.retain(|_, job| !job.is_expired() || matches!(job.stage, DispatchStage::Dispatched));
    }
}

#[cfg(test)]
mod tests {
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
}
