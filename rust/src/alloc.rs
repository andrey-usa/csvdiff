//! Allocations that can fail without taking the process with them.
//!
//! Rust's default allocator has no failure path. `Vec::with_capacity`, `vec![]`
//! and `Vec::reserve` all end in `handle_alloc_error`, which prints to stderr
//! from inside the allocator and calls `abort`. Every other port in this
//! repository answers a budget it cannot meet the same way it answers a missing
//! column — a line on stderr and exit 2:
//!
//! ```text
//! C     error: out of memory reading the parquet file
//! C++   error: std::bad_alloc
//! Zig   error: out of memory
//! ```
//!
//! This port aborted on signal 6 instead, and at fifty million rows of Parquet
//! that was the difference between a refusal and no answer at all: the three
//! that refuse still print a summary line and let the harness carry on to the
//! next port, and the one that aborts loses the run. Three threads racing into
//! `handle_alloc_error` also interleave their messages mid-word, so the one
//! line it did print was unreadable.
//!
//! So the allocations that scale with the input — one entry per row, per
//! distinct key, per uncompressed byte — go through the helpers here, which
//! return [`Error`] at the allocation that would have crossed the line. A
//! bounded allocation (a per-thread block, a header, a column name) does not
//! need them: it cannot be the one that fails, and routing it through here
//! would only add noise.
//!
//! The `what` label is not decoration. A refusal that names the structure is
//! the answer to *how much would be enough*, which is the question anyone
//! laddering `--memory-cap` is asking. `hash per row` and `the uncompressed
//! column` fail at very different budgets, and knowing which one hit the
//! ceiling is the difference between a number and a diagnosis.

use crate::{Error, Result};

/// The refusal itself: what was being sized, and how big it was.
///
/// Formatting allocates, which looks circular in a function that exists because
/// an allocation failed. It is not: the allocation that failed was megabytes and
/// has not been taken, so there is room for sixty bytes of message. The other
/// ports print at this point too.
fn refuse(what: &str, bytes: usize) -> Error {
    Error::new(format!(
        "out of memory: {what} needs {}",
        human_bytes(bytes)
    ))
}

fn human_bytes(bytes: usize) -> String {
    const MB: usize = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

/// `Vec::with_capacity(n)` that returns instead of aborting.
///
/// Exact, not amortised: these are sized from a row count that is already known,
/// and rounding up to the next power of two would be tens of megabytes of slack
/// at the sizes where the budget is tight.
pub fn sized<T>(n: usize, what: &str) -> Result<Vec<T>> {
    let mut v = Vec::new();
    grow(&mut v, n, what)?;
    Ok(v)
}

/// `vec![value; n]` that returns instead of aborting.
///
/// The fill is written rather than taken from the kernel, which is the same
/// choice [`crate::engine`]'s tables make deliberately: a fresh anonymous
/// mapping is one shared page of zeroes until something touches it, so a table
/// nothing reads before the inserts start would fault every page in under a
/// random probe.
pub fn filled<T: Clone>(value: T, n: usize, what: &str) -> Result<Vec<T>> {
    let mut v = sized(n, what)?;
    v.resize(n, value);
    Ok(v)
}

/// `vec.reserve_exact(extra)` that returns instead of aborting.
pub fn grow<T>(v: &mut Vec<T>, extra: usize, what: &str) -> Result<()> {
    v.try_reserve_exact(extra)
        .map_err(|_| refuse(what, extra.saturating_mul(size_of::<T>())))
}

/// Room for `extra` more entries in a vector that grows as it goes, amortised.
///
/// [`grow`] is exact, which is right when the final size is known and wrong
/// here: called once per block on a vector that is still being filled, an exact
/// reserve reallocates and copies on every call, so a column's worth of hits
/// would be quadratic. This asks for at least `extra` and lets the vector take
/// the usual doubling, so the checks are amortised to nothing.
pub fn room<T>(v: &mut Vec<T>, extra: usize, what: &str) -> Result<()> {
    if v.capacity() - v.len() >= extra {
        return Ok(());
    }
    v.try_reserve(extra).map_err(|_| {
        refuse(
            what,
            v.len().saturating_add(extra).saturating_mul(size_of::<T>()),
        )
    })
}

/// `vec.push(value)` that returns instead of aborting.
///
/// The check is the one `push` already makes; what changes is what happens when
/// it says the vector is full. It sits in the sweep's per-row loop, which is the
/// hottest loop in the port, and costs nothing measurable there: a compare
/// against a value already in a register, taken once per doubling.
#[inline]
pub fn push<T>(v: &mut Vec<T>, value: T, what: &str) -> Result<()> {
    if v.len() == v.capacity() {
        room(v, 1, what)?;
    }
    v.push(value);
    Ok(())
}

/// [`grow`] as a method, for the sites that read better that way.
pub trait TryGrow {
    /// Room for `extra` more entries, or a refusal naming `what`.
    fn try_grow(&mut self, extra: usize, what: &str) -> Result<()>;
}

impl<T> TryGrow for Vec<T> {
    fn try_grow(&mut self, extra: usize, what: &str) -> Result<()> {
        grow(self, extra, what)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sized_gives_at_least_what_was_asked() {
        let v: Vec<u64> = sized(1000, "test").unwrap();
        assert!(v.capacity() >= 1000);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn filled_writes_the_value() {
        let v = filled(7u32, 4, "test").unwrap();
        assert_eq!(v, vec![7, 7, 7, 7]);
    }

    #[test]
    fn grow_is_additive() {
        let mut v: Vec<u8> = vec![1, 2, 3];
        v.try_grow(100, "test").unwrap();
        assert!(v.capacity() >= 103);
        assert_eq!(v.len(), 3);
    }

    /// An allocation no machine can serve refuses rather than aborting. This is
    /// the whole point of the module: on the old code the process died here.
    #[test]
    fn an_impossible_size_refuses() {
        let huge = usize::MAX / 16;
        let e = sized::<u64>(huge, "hash per row").unwrap_err();
        let m = e.to_string();
        assert!(m.starts_with("out of memory: hash per row needs "), "{m}");
    }

    #[test]
    fn the_size_is_in_bytes_not_entries() {
        // 4 Mi entries of 8 bytes is 32 MB, not 4.
        let e = sized::<u64>(4 * 1024 * 1024 * 1024 * 1024, "pairs").unwrap_err();
        assert!(e.to_string().ends_with(" MB"), "{e}");
    }

    #[test]
    fn push_appends_and_grows() {
        let mut v: Vec<u32> = Vec::new();
        for i in 0..1000 {
            push(&mut v, i, "test").unwrap();
        }
        assert_eq!(v.len(), 1000);
        assert_eq!(v[999], 999);
    }

    #[test]
    fn room_is_amortised_not_exact() {
        // The point of `room` over `grow`: filling one entry at a time must not
        // reallocate every time, or a column's worth of hits is quadratic.
        let mut v: Vec<u64> = Vec::new();
        let mut reallocs = 0;
        let mut cap = v.capacity();
        for i in 0..10_000u64 {
            room(&mut v, 1, "test").unwrap();
            v.push(i);
            if v.capacity() != cap {
                cap = v.capacity();
                reallocs += 1;
            }
        }
        assert!(reallocs < 32, "{reallocs} reallocations for 10k pushes");
    }

    #[test]
    fn overflowing_the_byte_count_still_refuses() {
        // usize::MAX entries of 8 bytes overflows the multiplication; the
        // saturating form keeps it a refusal rather than a panic in debug.
        let e = sized::<u64>(usize::MAX, "pairs").unwrap_err();
        assert!(
            e.to_string().starts_with("out of memory: pairs needs "),
            "{e}"
        );
    }
}
