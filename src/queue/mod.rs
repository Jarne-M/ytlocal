//! The download queue: which jobs wait, which run, and how many at once.
//! The scheduler is plain data; the backend spawns the jobs it hands out.

pub mod job;
pub mod runner;

use std::collections::{HashMap, VecDeque};

use tokio_util::sync::CancellationToken;

use job::{JobId, JobSpec};

pub const DEFAULT_CONCURRENCY: usize = 2;
pub const MAX_CONCURRENCY: usize = 8;

#[derive(Debug)]
pub struct Scheduler {
    pending: VecDeque<JobSpec>,
    running: HashMap<JobId, CancellationToken>,
    concurrency: usize,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(DEFAULT_CONCURRENCY)
    }
}

impl Scheduler {
    pub fn new(concurrency: usize) -> Self {
        Self {
            pending: VecDeque::new(),
            running: HashMap::new(),
            concurrency: concurrency.clamp(1, MAX_CONCURRENCY),
        }
    }

    pub fn set_concurrency(&mut self, n: usize) {
        self.concurrency = n.clamp(1, MAX_CONCURRENCY);
    }

    /// Add a job (or re-add one for a retry). Ignored if it's already queued or running.
    pub fn enqueue(&mut self, spec: JobSpec) {
        if self.running.contains_key(&spec.id) || self.pending.iter().any(|j| j.id == spec.id) {
            return;
        }
        self.pending.push_back(spec);
    }

    /// Next job to start, if a slot is free. The token cancels it.
    pub fn start_next(&mut self) -> Option<(JobSpec, CancellationToken)> {
        if self.running.len() >= self.concurrency {
            return None;
        }
        let spec = self.pending.pop_front()?;
        let token = CancellationToken::new();
        self.running.insert(spec.id, token.clone());
        Some((spec, token))
    }

    /// Cancel a job. Returns true if it was still waiting (so nothing is
    /// running that will report the cancellation itself).
    pub fn cancel(&mut self, id: JobId) -> bool {
        if let Some(token) = self.running.get(&id) {
            token.cancel();
            return false;
        }
        let before = self.pending.len();
        self.pending.retain(|j| j.id != id);
        before != self.pending.len()
    }

    /// Cancel everything; returns the ids of jobs that were only waiting.
    pub fn cancel_all(&mut self) -> Vec<JobId> {
        for token in self.running.values() {
            token.cancel();
        }
        self.pending.drain(..).map(|j| j.id).collect()
    }

    pub fn finished(&mut self, id: JobId) {
        self.running.remove(&id);
    }

    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.running.is_empty()
    }

    pub fn running_count(&self) -> usize {
        self.running.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytdlp::command::{AudioFormat, Options, OutputFormat};
    use job::ConflictPolicy;

    fn job(id: JobId) -> JobSpec {
        JobSpec {
            id,
            url: String::new(),
            video_id: None,
            title: String::new(),
            channel: None,
            duration: None,
            album: None,
            format: OutputFormat::Audio {
                format: AudioFormat::Mp3,
                kbps: 192,
            },
            options: Options::default(),
            output_dir: Default::default(),
            template: String::new(),
            on_conflict: ConflictPolicy::Rename,
        }
    }

    #[test]
    fn respects_concurrency() {
        let mut s = Scheduler::new(2);
        for i in 1..=4 {
            s.enqueue(job(i));
        }
        assert_eq!(s.start_next().unwrap().0.id, 1);
        assert_eq!(s.start_next().unwrap().0.id, 2);
        assert!(s.start_next().is_none());
        s.finished(1);
        assert_eq!(s.start_next().unwrap().0.id, 3);
        s.set_concurrency(3);
        assert_eq!(s.start_next().unwrap().0.id, 4);
        assert!(s.start_next().is_none());
        assert!(!s.is_idle());
        for i in 2..=4 {
            s.finished(i);
        }
        assert!(s.is_idle());
    }

    #[test]
    fn cancel_pending_and_running() {
        let mut s = Scheduler::new(1);
        s.enqueue(job(1));
        s.enqueue(job(2));
        let (_, token) = s.start_next().unwrap();
        assert!(!s.cancel(1)); // running: cancelled via token
        assert!(token.is_cancelled());
        assert!(s.cancel(2)); // pending: removed
        assert!(!s.cancel(99));
        s.finished(1);
        assert!(s.is_idle());
    }

    #[test]
    fn cancel_all_and_dedup() {
        let mut s = Scheduler::new(1);
        s.enqueue(job(1));
        s.enqueue(job(1));
        s.enqueue(job(2));
        let (_, token) = s.start_next().unwrap();
        s.enqueue(job(1)); // running: ignored
        assert_eq!(s.cancel_all(), vec![2]);
        assert!(token.is_cancelled());
        assert_eq!(s.running_count(), 1);
    }

    #[test]
    fn concurrency_is_clamped() {
        let mut s = Scheduler::new(0);
        s.enqueue(job(1));
        assert!(s.start_next().is_some());
        s.set_concurrency(100);
        assert_eq!(s.concurrency, MAX_CONCURRENCY);
    }
}
