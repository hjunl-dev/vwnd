mod lbq;
mod worker_pool;

use core::fmt;
use std::{
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicUsize, Ordering},
};

// Errors

pub enum PushError<T> {
    Disposed(T),
    Full(T),
}

impl<T> PushError<T> {
    pub fn into_inner(self) -> T {
        match self {
            PushError::Disposed(t) | PushError::Full(t) => t,
        }
    }

    pub fn is_disposed(&self) -> bool {
        matches!(self, PushError::Disposed(_))
    }
}

impl<T> fmt::Debug for PushError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::Disposed(_) => f.write_str("PushError::Disposed"),
            PushError::Full(_) => f.write_str("PushError::Full"),
        }
    }
}

impl<T> fmt::Display for PushError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::Disposed(_) => f.write_str("queue is disposed"),
            PushError::Full(_) => f.write_str("queue is full"),
        }
    }
}

impl<T> std::error::Error for PushError<T> {}

// CachePadded to prevent false sharing

#[repr(align(128))]
#[derive(Debug, Default, Clone, Copy)]
struct CachePadded<T>(T);

impl<T> CachePadded<T> {
    pub const fn new(t: T) -> Self {
        Self(t)
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Condition variable waiter count

pub struct WaiterGuard<'a>(&'a AtomicUsize);

impl Drop for WaiterGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
pub struct CondWaiters(AtomicUsize);

impl CondWaiters {
    #[inline]
    pub const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    #[inline]
    #[must_use]
    pub fn enter(&self) -> WaiterGuard<'_> {
        self.0.fetch_add(1, Ordering::Relaxed);
        WaiterGuard(&self.0)
    }

    #[inline]
    pub fn any(&self) -> bool {
        self.0.load(Ordering::Relaxed) > 0
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

// blocking queue trait

pub trait BQ<T: Send>: Send + Sync {
    fn push(&self, item: T) -> Result<(), ()>;
    fn try_push(&self, item: T) -> Result<(), ()>;

    fn pop(&self) -> Result<T, ()>;
    fn try_pop(&self) -> Result<T, ()>;

    fn dispose(&self);

    fn capacity(&self) -> usize;

    fn len(&self) -> usize;

    fn is_disposed(&self) -> bool;
}

pub type Job = Box<dyn FnOnce() + Send + 'static>;

pub trait Executor: Send + Sync {
    fn submit(&self, job: Job) -> Result<(), ()>;
    fn try_submit(&self, job: Job) -> Result<(), ()>;
    fn dispose(&self);

    fn is_disposed(&self) -> bool;
    fn worker_count(&self) -> usize;
}
