//! Fork (`fork/perf-portrait-draft.md`): a byte budget for images the renderers keep beside a view's stages, across
//! every view. Each kept image sits in a slot; the budget empties the slots it holds the longest without use once
//! they add up past its cap, and forgets a slot whose owner (a stage cache's entry) is gone. The image before the
//! portrait retouch step is kept this way: on the host ([`HOST`]) by the CPU renderer and the GPU renderer, and on the
//! device by the GPU renderer.

use std::sync::{Arc, Mutex, PoisonError, Weak};

/// What a budget can empty.
pub trait Evict: Send + Sync {
    /// Drop what the slot holds.
    fn evict(&self);
    /// Whether it still holds something.
    fn held(&self) -> bool;
}

/// One kept value: emptied by its budgets, dropped with its owner.
pub struct Slot<T>(Mutex<Option<Arc<T>>>);

impl<T: Send + Sync> Evict for Slot<T> {
    fn evict(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn held(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }
}

/// The bytes a set of slots may hold together.
pub struct Budget {
    cap: usize,
    /// Oldest use first: the slot and its bytes.
    list: Mutex<Vec<(Weak<dyn Evict>, usize)>>,
}

/// The host's budget for kept images before the retouch step (both renderers).
pub static HOST: Budget = Budget::new(512 << 20);

fn same(a: &Weak<dyn Evict>, b: &Weak<dyn Evict>) -> bool {
    std::ptr::addr_eq(a.as_ptr(), b.as_ptr())
}

impl Budget {
    pub const fn new(cap: usize) -> Budget {
        Budget { cap, list: Mutex::new(Vec::new()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(Weak<dyn Evict>, usize)>> {
        self.list.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bytes its live, held slots hold.
    pub fn bytes(&self) -> usize {
        self.lock().iter().filter(|(w, _)| w.upgrade().is_some_and(|s| s.held())).map(|(_, b)| b).sum()
    }

    /// Hold `slot` (`bytes`) as the newest, then empty the oldest slots until the rest fits (a slot larger than the
    /// whole budget is emptied at once).
    fn add(&self, slot: Weak<dyn Evict>, bytes: usize) {
        let mut list = self.lock();
        list.retain(|(w, _)| !same(w, &slot) && w.upgrade().is_some_and(|s| s.held()));
        list.push((slot, bytes));
        let mut total: usize = list.iter().map(|(_, b)| *b).fold(0, usize::saturating_add);
        while total > self.cap && !list.is_empty() {
            let (w, b) = list.remove(0);
            if let Some(s) = w.upgrade() {
                s.evict();
            }
            total = total.saturating_sub(b);
        }
    }

    /// Mark `slot` used now: the last to be emptied.
    fn touch(&self, slot: &Weak<dyn Evict>) {
        let mut list = self.lock();
        if let Some(i) = list.iter().position(|(w, _)| same(w, slot)) {
            let e = list.remove(i);
            list.push(e);
        }
    }
}

/// A value kept under one or more budgets (bytes per budget).
pub struct Kept<T> {
    slot: Arc<Slot<T>>,
    budgets: Vec<&'static Budget>,
}

impl<T> Clone for Kept<T> {
    fn clone(&self) -> Self {
        Kept { slot: self.slot.clone(), budgets: self.budgets.clone() }
    }
}

impl<T: Send + Sync + 'static> Kept<T> {
    /// Keep `value`, counting `bytes` against each budget.
    pub fn new(value: Arc<T>, under: &[(&'static Budget, usize)]) -> Kept<T> {
        let slot = Arc::new(Slot(Mutex::new(Some(value))));
        let weak: Weak<dyn Evict> = Arc::downgrade(&(slot.clone() as Arc<dyn Evict>));
        for (b, bytes) in under {
            b.add(weak.clone(), *bytes);
        }
        Kept { slot, budgets: under.iter().map(|(b, _)| *b).collect() }
    }

    /// The value, unless a budget emptied it; a value read is the last its budgets empty.
    pub fn get(&self) -> Option<Arc<T>> {
        let v = self.slot.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if v.is_some() {
            let weak: Weak<dyn Evict> = Arc::downgrade(&(self.slot.clone() as Arc<dyn Evict>));
            for b in &self.budgets {
                b.touch(&weak);
            }
        }
        v
    }

    /// The value without marking it used (memory reports).
    pub fn peek(&self) -> Option<Arc<T>> {
        self.slot.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_unused_values_go_first_and_owners_free_theirs() {
        static B: Budget = Budget::new(100);
        let a = Kept::new(Arc::new(1u8), &[(&B, 40)]);
        let b = Kept::new(Arc::new(2u8), &[(&B, 40)]);
        assert_eq!(B.bytes(), 80);
        // (a is read: b is now the oldest)
        assert!(a.get().is_some());
        let c = Kept::new(Arc::new(3u8), &[(&B, 40)]);
        assert!(a.peek().is_some() && b.peek().is_none() && c.peek().is_some(), "b went");
        assert_eq!(B.bytes(), 80);
        drop(a);
        assert_eq!(B.bytes(), 40, "a dropped with its owner");
        let big = Kept::new(Arc::new(4u8), &[(&B, 500)]);
        assert!(big.get().is_none(), "larger than the budget: not kept");
        assert!(c.get().is_none(), "and it made room first");
        assert_eq!(B.bytes(), 0);
    }

    #[test]
    fn a_value_under_two_budgets_goes_when_either_is_full() {
        static H: Budget = Budget::new(100);
        static D: Budget = Budget::new(50);
        let a = Kept::new(Arc::new(1u8), &[(&H, 30), (&D, 30)]);
        let b = Kept::new(Arc::new(2u8), &[(&H, 30)]);
        let c = Kept::new(Arc::new(3u8), &[(&H, 30), (&D, 30)]);
        assert!(a.peek().is_none() && b.peek().is_some() && c.peek().is_some());
        assert_eq!((H.bytes(), D.bytes()), (60, 30));
    }
}
