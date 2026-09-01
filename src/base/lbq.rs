use std::{
    ptr::NonNull,
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::base::{BQ, CachePadded, CondWaiters};

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

pub struct LinkedBQ<T> {
    capacity: usize,
    count: CachePadded<AtomicUsize>,
    disposed: CachePadded<AtomicBool>,
    pop_side: CachePadded<PopSide<T>>,
    push_side: CachePadded<PushSide<T>>,
}

impl<T> LinkedBQ<T> {
    pub fn new(capacity: usize) -> Self {
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
}

impl<T: Send> BQ<T> for LinkedBQ<T> {
    fn push(&self, item: T) -> Result<(), super::PushError<T>> {
        todo!()
    }

    fn try_push(&self, item: T) -> Result<(), super::PushError<T>> {
        todo!()
    }

    fn pop(&self) -> Result<T, super::PopError> {
        todo!()
    }

    fn try_pop(&self) -> Result<T, super::PopError> {
        todo!()
    }

    fn dispose(&self) {
        todo!()
    }

    fn capacity(&self) -> usize {
        todo!()
    }

    fn len(&self) -> usize {
        todo!()
    }

    fn is_disposed(&self) -> bool {
        todo!()
    }
}

impl<T> Drop for LinkedBQ<T> {
    fn drop(&mut self) {
        let mut curr = Some(self.pop_lock().0);
        while let Some(node) = curr {
            let boxed = unsafe { Box::from_raw(node.as_ptr()) };
            curr = boxed.next;
        }
    }
}

impl<T> std::fmt::Debug for LinkedBQ<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkedBQ")
            .field("capacity", &self.capacity)
            .field("len", &self.count.load(Ordering::Relaxed))
            .field("disposed", &self.disposed.load(Ordering::Relaxed))
            .finish()
    }
}

unsafe impl<T: Send> Send for LinkedBQ<T> {}
unsafe impl<T: Send> Sync for LinkedBQ<T> {}
