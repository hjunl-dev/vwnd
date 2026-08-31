use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    thread::JoinHandle,
};

use crate::base::{BQ, Executor, Job, lbq::LinkedBQ};

pub struct WorkerPool {
    job_q: Arc<dyn BQ<Job>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    num_workers: usize,
    disposed: AtomicBool,
}

impl WorkerPool {
    pub fn new(num_workers: usize, job_q_size: usize) -> Self {
        Self::with_job_q(num_workers, Arc::new(LinkedBQ::new(job_q_size)))
    }

    pub fn with_job_q(num_workers: usize, job_q: Arc<dyn BQ<Job>>) -> Self {
        todo!()
    }
}

impl Executor for WorkerPool {
    fn submit(&self, job: Job) -> Result<(), super::PushError<Job>> {
        todo!()
    }

    fn try_submit(&self, job: Job) -> Result<(), super::PushError<Job>> {
        todo!()
    }

    fn dispose(&self) {
        todo!()
    }

    fn is_disposed(&self) -> bool {
        todo!()
    }

    fn worker_count(&self) -> usize {
        todo!()
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.dispose();
    }
}
