//! The working memory of one call: every buffer that scales with the input is charged before it
//! is allocated, by all threads of the call together, and released when its guard drops.

use crate::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};

/// Booked at the start of every call for the small, short-lived allocations that do not scale
/// with the input and are not charged one by one (for example the boxed `io::Error` that
/// deflate-rs makes when the IDAT reader reports a bad chunk). A `max_alloc` below this is
/// `Error::AllocLimit` at once.
pub(crate) const SMALL: usize = 1024;

/// Bytes booked against `Limits::max_alloc`.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    max: AtomicU64,
    used: AtomicU64,
    peak: AtomicU64,
}

impl Ledger {
    /// Starts a call: `max` bytes allowed, `preload` of them already taken by the caller, plus
    /// [`SMALL`] for the small allocations that are not charged one by one.
    pub(crate) fn reset(&self, max: usize, preload: usize) -> Result<(), Error> {
        self.max.store(max as u64, Ordering::Relaxed);
        self.used.store(0, Ordering::Relaxed);
        self.peak.store(0, Ordering::Relaxed);
        self.reserve(preload.checked_add(SMALL).ok_or(Error::AllocLimit)?)
    }

    /// What may still be charged.
    pub(crate) fn left(&self) -> usize {
        let free = self
            .max
            .load(Ordering::Relaxed)
            .saturating_sub(self.used.load(Ordering::Relaxed));
        usize::try_from(free).unwrap_or(usize::MAX)
    }

    /// The most bytes booked at once in this call.
    #[cfg(test)]
    pub(crate) fn peak(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }

    /// Books `n` bytes for the rest of the call.
    pub(crate) fn reserve(&self, n: usize) -> Result<(), Error> {
        let n = n as u64;
        let max = self.max.load(Ordering::Relaxed);
        let r = self
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                u.checked_add(n).filter(|&t| t <= max)
            });
        match r {
            Ok(u) => {
                self.peak.fetch_max(u + n, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => Err(Error::AllocLimit),
        }
    }

    /// Books `n` bytes until the returned guard drops.
    pub(crate) fn charge(&self, n: usize) -> Result<Held<'_>, Error> {
        self.reserve(n)?;
        Ok(Held { l: self, n })
    }

    fn release(&self, n: usize) {
        self.used.fetch_sub(n as u64, Ordering::AcqRel);
    }
}

/// Bytes booked in a [`Ledger`], released on drop.
#[derive(Debug)]
pub(crate) struct Held<'a> {
    l: &'a Ledger,
    n: usize,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.l.release(self.n);
    }
}

/// Sets `v` to `n` zero bytes and books `n` bytes for the rest of the call. A buffer that is
/// already big enough (a `Decoder` kept it) allocates nothing.
pub(crate) fn zeroed(v: &mut Vec<u8>, n: usize, ledger: &Ledger) -> Result<(), Error> {
    ledger.reserve(n)?;
    v.clear();
    if n > v.capacity() {
        v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
    }
    v.resize(n, 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_release_and_peak() {
        let l = Ledger::default();
        l.reset(SMALL + 100, 0).unwrap();
        let a = l.charge(60).unwrap();
        assert_eq!(l.charge(50).unwrap_err(), Error::AllocLimit);
        assert_eq!(l.left(), 40);
        drop(a);
        assert_eq!(l.left(), 100);
        let _b = l.charge(100).unwrap();
        assert_eq!(l.peak(), SMALL as u64 + 100);
    }

    #[test]
    fn reset_preload_and_zeroed() {
        let l = Ledger::default();
        assert_eq!(l.reset(SMALL + 10, 11).unwrap_err(), Error::AllocLimit);
        l.reset(SMALL + 10, 4).unwrap();
        assert_eq!(l.left(), 6);
        let mut v = Vec::new();
        assert_eq!(zeroed(&mut v, 7, &l).unwrap_err(), Error::AllocLimit);
        zeroed(&mut v, 6, &l).unwrap();
        assert_eq!(v, [0; 6]);
        assert_eq!(l.left(), 0);
        // A new call with the buffer kept: no allocation, same charge.
        l.reset(SMALL + 10, 4).unwrap();
        let cap = v.capacity();
        zeroed(&mut v, 6, &l).unwrap();
        assert_eq!(v.capacity(), cap);
        assert_eq!(l.left(), 0);
    }
}
