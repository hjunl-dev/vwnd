use std::{
    ptr::NonNull,
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::base::{BQ, CachePadded, CondWaiters, PopError, PushError};

// ============================================================
// Primitives for building LBQ
// ============================================================

struct Node<T> {
    item: Option<T>,
    next: Option<NonNull<Node<T>>>,
}

impl<T> Node<T> {
    fn new(item: Option<T>) -> NonNull<Self> {
        let ptr = Box::into_raw(Box::new(Node { item, next: None }));
        // safety: Box never returns nullptr
        unsafe { NonNull::new_unchecked(ptr) }
    }

    fn dummy() -> NonNull<Self> {
        Self::new(None)
    }
}

struct NodePtr<T>(NonNull<Node<T>>);

unsafe impl<T: Send> Send for NodePtr<T> {}

struct PopSide<T> {
    head: Mutex<NodePtr<T>>,
    not_empty: Condvar,
    pop_waiters: CondWaiters,
}

struct PushSide<T> {
    tail: Mutex<NodePtr<T>>,
    not_full: Condvar,
    push_waiters: CondWaiters,
}

// ============================================================
// LBQ
// ============================================================

pub struct LBQ<T> {
    capacity: usize,
    count: CachePadded<AtomicUsize>,
    disposed: CachePadded<AtomicBool>,
    pop_side: CachePadded<PopSide<T>>,
    push_side: CachePadded<PushSide<T>>,
}

impl<T> LBQ<T> {
    fn new(capacity: usize) -> Self {
        let dummy = Node::dummy();
        let capacity = if capacity == 0 { usize::MAX } else { capacity };
        Self {
            capacity,
            count: CachePadded::new(AtomicUsize::new(0)),
            disposed: CachePadded::new(AtomicBool::new(false)),
            pop_side: CachePadded::new(PopSide {
                head: Mutex::new(NodePtr(dummy)),
                not_empty: Condvar::new(),
                pop_waiters: CondWaiters::new(),
            }),
            push_side: CachePadded::new(PushSide {
                tail: Mutex::new(NodePtr(dummy)),
                not_full: Condvar::new(),
                push_waiters: CondWaiters::new(),
            }),
        }
    }

    pub fn unbounded() -> Self {
        Self::new(0)
    }

    pub fn bounded(capacity: usize) -> Self {
        // capacity > 0
        Self::new(capacity.max(1))
    }

    #[inline]
    fn pop_lock(&self) -> MutexGuard<'_, NodePtr<T>> {
        self.pop_side
            .head
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[inline]
    fn push_lock(&self) -> MutexGuard<'_, NodePtr<T>> {
        self.push_side
            .tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.count.load(Ordering::Acquire) >= self.capacity
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }

    fn en_q(&self, item: T, mut guard: MutexGuard<'_, NodePtr<T>>) {
        let node = Node::new(Some(item));

        unsafe {
            (*guard.0.as_ptr()).next = Some(node);
        }
        guard.0 = node;

        let prev = self.count.fetch_add(1, Ordering::AcqRel);
        // cascading signal (notify to producer)
        if prev + 1 < self.capacity && self.push_side.push_waiters.any() {
            self.push_side.not_full.notify_one();
        }
        drop(guard);
        // was empty (notify to consumer, empty -> not empty)
        if prev == 0 {
            let _g = self.pop_lock();
            if self.pop_side.pop_waiters.any() {
                self.pop_side.not_empty.notify_one();
            }
        }
    }

    fn de_q(&self, mut guard: MutexGuard<'_, NodePtr<T>>) -> T {
        let old = guard.0;
        let first = unsafe { (*old.as_ptr()).next.unwrap_unchecked() };
        guard.0 = first;
        let item = unsafe {
            drop(Box::from_raw(old.as_ptr()));
            (*first.as_ptr()).item.take().unwrap_unchecked()
        };

        let prev = self.count.fetch_sub(1, Ordering::AcqRel);
        // cascading signal (notify to consumer)
        if prev > 1 && self.pop_side.pop_waiters.any() {
            self.pop_side.not_empty.notify_one();
        }
        drop(guard);
        // was full (notify to producer, full -> not full)
        if prev == self.capacity {
            let _g = self.push_lock();
            if self.push_side.push_waiters.any() {
                self.push_side.not_full.notify_one();
            }
        }
        item
    }
}

impl<T: Send> BQ<T> for LBQ<T> {
    fn push(&self, item: T) -> Result<(), PushError<T>> {
        let mut g = self.push_lock();

        if !self.is_disposed() && self.is_full() {
            let _wg = self.push_side.push_waiters.enter();
            g = self
                .push_side
                .not_full
                .wait_while(g, |_g| !self.is_disposed() && self.is_full())
                .unwrap_or_else(PoisonError::into_inner);
        }
        if self.is_disposed() {
            return Err(PushError::Disposed(item));
        }
        self.en_q(item, g);
        Ok(())
    }

    fn try_push(&self, item: T) -> Result<(), PushError<T>> {
        let g = self.push_lock();

        if self.is_disposed() {
            return Err(PushError::Disposed(item));
        }
        if self.is_full() {
            return Err(PushError::Full(item));
        }
        self.en_q(item, g);
        Ok(())
    }

    fn pop(&self) -> Result<T, PopError> {
        let mut g = self.pop_lock();

        if !self.is_disposed() && self.is_empty() {
            let _wg = self.pop_side.pop_waiters.enter();
            g = self
                .pop_side
                .not_empty
                .wait_while(g, |_g| !self.is_disposed() && self.is_empty())
                .unwrap_or_else(PoisonError::into_inner);
        }
        if self.is_empty() {
            return Err(PopError::Disposed);
        }
        Ok(self.de_q(g))
    }

    fn try_pop(&self) -> Result<T, PopError> {
        let g = self.pop_lock();

        if self.is_empty() {
            return Err(if self.is_disposed() {
                PopError::Disposed
            } else {
                PopError::Empty
            });
        }
        Ok(self.de_q(g))
    }

    fn dispose(&self) {
        if self
            .disposed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            {
                let _g = self.pop_lock();
                self.pop_side.not_empty.notify_all();
            }
            {
                let _g = self.push_lock();
                self.push_side.not_full.notify_all();
            }
        }
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn len(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }
}

impl<T> Drop for LBQ<T> {
    fn drop(&mut self) {
        let mut curr = Some(self.pop_lock().0);
        while let Some(node) = curr {
            let boxed = unsafe { Box::from_raw(node.as_ptr()) };
            curr = boxed.next;
        }
    }
}

impl<T> std::fmt::Debug for LBQ<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LBQ")
            .field("capacity", &self.capacity)
            .field("len", &self.count.load(Ordering::Relaxed))
            .field("disposed", &self.disposed.load(Ordering::Relaxed))
            .finish()
    }
}

unsafe impl<T: Send> Send for LBQ<T> {}
unsafe impl<T: Send> Sync for LBQ<T> {}
