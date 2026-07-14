//! Pending coding-job state machine.
//!
//! A `PendingDevelopmentJob` is created when the LLM calls `prepare_feature_development`
//! and destroyed when it is dispatched, cancelled, or expires. Expiry is 15 minutes.
//! Jobs are held in memory only; no persistence across restarts (acceptable because no
//! remote job starts before the owner confirms dispatch).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::catalog::CodingAgent;

const EXPIRY: Duration = Duration::from_secs(15 * 60);

/// The structured specification built from the LLM tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevelopmentSpecification {
    pub title: String,
    pub objective: String,
    pub context: String,
    pub requirements: Vec<String>,
    pub acceptance_criteria: Vec<String>,
}

/// Partially-filled agent/model/effort selection built during the Discord component flow.
#[derive(Debug, Clone, Default)]
pub struct PartialAgentSelection {
    pub agent: Option<CodingAgent>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// Stages of the multi-step Discord component flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStage {
    ChoosingAgent,
    ChoosingModel,
    ChoosingEffort,
    Confirming,
    /// Atomic transition: issue is being created.
    Dispatching,
    /// Issue created successfully.
    Dispatched,
    Cancelled,
}

/// A pending coding job awaiting owner confirmation.
pub struct PendingDevelopmentJob {
    pub id: Uuid,
    pub owner_id: u64,
    pub channel_id: u64,
    /// The Discord message ID of the component UI message (set after initial send).
    pub message_id: Option<u64>,
    pub specification: DevelopmentSpecification,
    pub selection: PartialAgentSelection,
    pub stage: DispatchStage,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl PendingDevelopmentJob {
    pub fn new(owner_id: u64, channel_id: u64, spec: DevelopmentSpecification) -> Self {
        let now = Utc::now();
        let expires_at = now
            + chrono::Duration::from_std(EXPIRY).expect("expiry duration fits in chrono::Duration");
        Self {
            id: Uuid::new_v4(),
            owner_id,
            channel_id,
            message_id: None,
            specification: spec,
            selection: PartialAgentSelection::default(),
            stage: DispatchStage::ChoosingAgent,
            created_at: now,
            expires_at,
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
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

    /// Atomically transition a `Confirming` job to `Dispatching`.
    ///
    /// Returns `true` exactly once per job (prevents double-dispatch from
    /// rapid-clicking the Dispatch button).
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
            job.stage = DispatchStage::Confirming;
        }
    }

    pub fn cancel(&self, id: Uuid) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id) {
            job.stage = DispatchStage::Cancelled;
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

    fn spec() -> DevelopmentSpecification {
        DevelopmentSpecification {
            title: "Test".into(),
            objective: "Do something".into(),
            context: String::new(),
            requirements: vec!["Req 1".into()],
            acceptance_criteria: vec!["AC 1".into()],
        }
    }

    #[test]
    fn new_job_starts_in_choosing_agent_stage() {
        let job = PendingDevelopmentJob::new(1, 2, spec());
        assert_eq!(job.stage, DispatchStage::ChoosingAgent);
        assert!(!job.is_expired());
    }

    #[test]
    fn insert_and_retrieve() {
        let store = PendingJobStore::default();
        let job = PendingDevelopmentJob::new(1, 2, spec());
        let id = store.insert(job);
        let found = store.with_job(id, |j| j.owner_id);
        assert_eq!(found, Some(1));
    }

    #[test]
    fn with_job_mut_allows_stage_update() {
        let store = PendingJobStore::default();
        let job = PendingDevelopmentJob::new(1, 2, spec());
        let id = store.insert(job);
        store.with_job_mut(id, |j| j.stage = DispatchStage::ChoosingModel);
        let stage = store.with_job(id, |j| j.stage);
        assert_eq!(stage, Some(DispatchStage::ChoosingModel));
    }

    #[test]
    fn try_start_dispatch_only_succeeds_once() {
        let store = PendingJobStore::default();
        let job = PendingDevelopmentJob::new(1, 2, spec());
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
    fn mark_dispatched_changes_stage() {
        let store = PendingJobStore::default();
        let job = PendingDevelopmentJob::new(1, 2, spec());
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
        let job = PendingDevelopmentJob::new(1, 2, spec());
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
        let job = PendingDevelopmentJob::new(1, 2, spec());
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
        let mut job = PendingDevelopmentJob::new(1, 2, spec());
        // Force expiry.
        job.expires_at = Utc::now() - chrono::Duration::seconds(1);
        let id = store.insert(job);
        store.evict_expired();
        assert!(store.with_job(id, |_| ()).is_none());
    }

    #[test]
    fn evict_expired_keeps_dispatched_jobs() {
        let store = PendingJobStore::default();
        let mut job = PendingDevelopmentJob::new(1, 2, spec());
        job.expires_at = Utc::now() - chrono::Duration::seconds(1);
        let id = store.insert(job);
        store.with_job_mut(id, |j| j.stage = DispatchStage::Dispatched);
        store.evict_expired();
        assert!(store.with_job(id, |_| ()).is_some());
    }
}
