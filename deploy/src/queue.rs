//! Per-application serialization (spec D4): one deployment runs at a time; a newer
//! *queued* request supersedes an older queued one; a running deployment is never
//! interrupted by the queue. Superseded requests are reported to the caller so it can
//! record them (as `superseded`), never silently dropped.

use crate::ids::DeploymentId;

#[derive(Debug, Default)]
pub struct AppQueue {
    running: Option<DeploymentId>,
    queued: Option<DeploymentId>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Enqueued {
    /// The request may start right now.
    pub start_now: bool,
    /// A previously queued request replaced by this one.
    pub superseded: Option<DeploymentId>,
}

impl AppQueue {
    pub fn new() -> AppQueue {
        AppQueue::default()
    }

    pub fn running(&self) -> Option<&DeploymentId> {
        self.running.as_ref()
    }

    pub fn queued(&self) -> Option<&DeploymentId> {
        self.queued.as_ref()
    }

    pub fn enqueue(&mut self, id: DeploymentId) -> Enqueued {
        if self.running.is_none() {
            self.running = Some(id);
            return Enqueued {
                start_now: true,
                superseded: None,
            };
        }
        let superseded = self.queued.replace(id);
        Enqueued {
            start_now: false,
            superseded,
        }
    }

    /// The running deployment ended. Returns the queued request that may start next.
    pub fn finish(&mut self, id: &DeploymentId) -> Option<DeploymentId> {
        if self.running.as_ref() != Some(id) {
            return None; // not the running one: ignore rather than disturb the queue
        }
        self.running = self.queued.take();
        self.running.clone()
    }

    /// Remove a queued request that was cancelled before it started.
    pub fn cancel_queued(&mut self, id: &DeploymentId) -> bool {
        if self.queued.as_ref() == Some(id) {
            self.queued = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::IdGenerator;
    use crate::time::{ManualClock, SeqEntropy};

    fn ids(n: usize) -> Vec<DeploymentId> {
        let mut g = IdGenerator::new(ManualClock::new(1), SeqEntropy::new(0));
        (0..n).map(|_| g.deployment().unwrap()).collect()
    }

    #[test]
    fn first_request_starts_immediately() {
        let i = ids(1);
        let mut q = AppQueue::new();
        assert_eq!(
            q.enqueue(i[0].clone()),
            Enqueued {
                start_now: true,
                superseded: None
            }
        );
        assert_eq!(q.running(), Some(&i[0]));
    }

    #[test]
    fn newest_queued_request_supersedes_older_queued_but_never_the_running_one() {
        let i = ids(4);
        let mut q = AppQueue::new();
        q.enqueue(i[0].clone());
        assert_eq!(
            q.enqueue(i[1].clone()),
            Enqueued {
                start_now: false,
                superseded: None
            }
        );
        assert_eq!(
            q.enqueue(i[2].clone()),
            Enqueued {
                start_now: false,
                superseded: Some(i[1].clone())
            }
        );
        assert_eq!(
            q.enqueue(i[3].clone()),
            Enqueued {
                start_now: false,
                superseded: Some(i[2].clone())
            }
        );
        assert_eq!(q.running(), Some(&i[0]), "the running deployment is untouched");
        assert_eq!(q.queued(), Some(&i[3]));
    }

    #[test]
    fn finishing_promotes_the_queued_request() {
        let i = ids(2);
        let mut q = AppQueue::new();
        q.enqueue(i[0].clone());
        q.enqueue(i[1].clone());
        assert_eq!(q.finish(&i[0]), Some(i[1].clone()));
        assert_eq!(q.running(), Some(&i[1]));
        assert_eq!(q.queued(), None);
        assert_eq!(q.finish(&i[1]), None);
        assert_eq!(q.running(), None);
    }

    #[test]
    fn finishing_a_deployment_that_is_not_running_changes_nothing() {
        let i = ids(3);
        let mut q = AppQueue::new();
        q.enqueue(i[0].clone());
        q.enqueue(i[1].clone());
        assert_eq!(q.finish(&i[2]), None);
        assert_eq!(q.running(), Some(&i[0]));
        assert_eq!(q.queued(), Some(&i[1]));
    }

    #[test]
    fn cancelling_a_queued_request_leaves_the_running_one_alone() {
        let i = ids(2);
        let mut q = AppQueue::new();
        q.enqueue(i[0].clone());
        q.enqueue(i[1].clone());
        assert!(
            !q.cancel_queued(&i[0]),
            "the running one cannot be cancelled through the queue"
        );
        assert!(q.cancel_queued(&i[1]));
        assert_eq!(q.queued(), None);
    }
}
