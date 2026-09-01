use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use crate::base::{BQ, Executor, Job, PushError, lbq::LinkedBQ};

// ============================================================
// WorkerPool
// ============================================================

thread_local! {
    static IN_WORKER: Cell<bool> = const { Cell::new(false) };
}

const WORKER_STACK_SIZE: usize = 256 * 1024;

fn panic_message(e: &(dyn std::any::Any + Send)) -> &str {
    if let Some(s) = e.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.as_str()
    } else {
        "non-string panic message"
    }
}

fn worker_panic_message(e: &(dyn std::any::Any + Send)) {
    let msg = panic_message(&*e);
    eprintln!(
        "[{}] worker panicked: {}",
        thread::current()
            .name()
            .unwrap_or(format!("worker-{:?}", thread::current().id()).as_str()),
        msg
    );
}

fn worker_loop(job_q: Arc<dyn BQ<Job>>) {
    IN_WORKER.with(|b| b.set(true));

    while let Ok(job) = job_q.pop() {
        if let Err(e) = catch_unwind(AssertUnwindSafe(move || job())) {
            worker_panic_message(&*e);
        }
    }
}

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
        let num_workers = Self::ensure_num_workers(num_workers);
        let mut workers = Vec::with_capacity(num_workers);

        for i in 0..num_workers {
            let job_q_clone = Arc::clone(&job_q);
            workers.push(
                thread::Builder::new()
                    .name(format!("pool-worker-{i}"))
                    .stack_size(WORKER_STACK_SIZE)
                    .spawn(move || worker_loop(job_q_clone))
                    .expect("failed to spawn worker thread"),
            );
        }
        Self {
            job_q,
            workers: Mutex::new(workers),
            num_workers,
            disposed: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn ensure_num_workers(num_workers: usize) -> usize {
        if num_workers == 0 {
            thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        } else {
            num_workers
        }
    }

    #[inline]
    pub fn queued(&self) -> usize {
        self.job_q.len()
    }

    /// Used in a static global pool within a DLL to prevent deadlocks in DllMain.
    /// Similar to C++ 'NoDestructor<T>'.
    ///
    /// Scenario:
    /// DLL_PROCESS_DETACH -> static free (Drop) -> thread join -> DLL_THREAD_DETACH -> deadlock
    ///
    /// Main Thread (DLL_PROCESS_DETACH, holding Loader Lock) -> waiting for the worker thread to join.
    /// Worker Thread (exiting, requires DLL_THREAD_DETACH) -> waiting to acquire the Loader Lock.
    #[inline]
    pub fn leak(self) -> &'static Self {
        Box::leak(Box::new(self))
    }
}

impl Executor for WorkerPool {
    /// Prevent permanent waiting when the job queue is full and a submission is made from within a worker 
    /// (use inline execution, caller-runs).
    fn submit(&self, job: Job) -> Result<(), PushError<Job>> {
        if IN_WORKER.with(|f| f.get()) {
            return match self.job_q.try_push(job) {
                Err(PushError::Full(job)) => {
                    job();
                    Ok(())
                }
                other => other,
            };
        }
        self.job_q.push(job)
    }

    fn try_submit(&self, job: Job) -> Result<(), PushError<Job>> {
        self.job_q.try_push(job)
    }

    fn dispose(&self) {
        if self
            .disposed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let workers: Vec<JoinHandle<()>> = {
                let mut g = self.workers.lock().unwrap_or_else(PoisonError::into_inner);
                g.drain(..).collect()
            };

            let caller = thread::current().id();
            for w in workers {
                if w.thread().id() == caller {
                    continue;
                }
                if let Err(e) = w.join() {
                    worker_panic_message(&*e);
                }
            }
        }
    }

    fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }

    fn worker_count(&self) -> usize {
        self.num_workers
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.dispose();
    }
}
