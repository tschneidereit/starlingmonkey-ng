// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! State scoped to the runtime that owns it: one wasm instance, or one native thread.
//!
//! Under the component model a task is a thread, and calling an async export
//! spawns one, so on WASIp3 every export call into an instance starts with fresh
//! thread-local storage. State a runtime keeps between calls cannot live in a
//! `thread_local!` there. Linear memory does persist across calls, so a `static`
//! holds.
//!
//! Native targets keep `thread_local!`, so runtimes on separate threads stay
//! independent of each other.

/// A `static` standing in for a `thread_local!` on wasm targets.
///
/// The value is built on first access, as a `thread_local!` builds its own. It is
/// never dropped: an instance has no equivalent of a thread exit.
#[cfg(target_family = "wasm")]
pub struct InstanceLocal<T> {
    cell: std::cell::OnceCell<T>,
    init: fn() -> T,
}

#[cfg(target_family = "wasm")]
// SAFETY: a wasm instance runs guest code on one thread. Concurrent export calls
// interleave at await points rather than running in parallel, which is the access
// pattern several requests already have against a thread-local in one instance on
// wasm32-wasip2.
unsafe impl<T> Sync for InstanceLocal<T> {}

#[cfg(target_family = "wasm")]
impl<T> InstanceLocal<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Self {
            cell: std::cell::OnceCell::new(),
            init,
        }
    }

    /// Run `f` against the value, matching `LocalKey::with`.
    pub fn with<R>(&'static self, f: impl FnOnce(&T) -> R) -> R {
        f(self.cell.get_or_init(self.init))
    }

    /// Matching `LocalKey::try_with`, which fails only once a thread has destroyed
    /// its copy. Nothing destroys this one, so `f` always runs.
    pub fn try_with<R>(
        &'static self,
        f: impl FnOnce(&T) -> R,
    ) -> Result<R, std::convert::Infallible> {
        Ok(self.with(f))
    }
}

/// The `Cell` conveniences `LocalKey` offers, so call sites read the same either way.
#[cfg(target_family = "wasm")]
impl<T> InstanceLocal<std::cell::Cell<T>> {
    pub fn set(&'static self, value: T) {
        self.with(|cell| cell.set(value));
    }

    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }

    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(std::cell::Cell::take)
    }
}

#[cfg(target_family = "wasm")]
impl<T: Copy> InstanceLocal<std::cell::Cell<T>> {
    pub fn get(&'static self) -> T {
        self.with(std::cell::Cell::get)
    }
}

/// The `RefCell` conveniences `LocalKey` offers.
#[cfg(target_family = "wasm")]
impl<T> InstanceLocal<std::cell::RefCell<T>> {
    pub fn with_borrow<R>(&'static self, f: impl FnOnce(&T) -> R) -> R {
        self.with(|cell| f(&cell.borrow()))
    }

    pub fn with_borrow_mut<R>(&'static self, f: impl FnOnce(&mut T) -> R) -> R {
        self.with(|cell| f(&mut cell.borrow_mut()))
    }
}

/// Declare state scoped to one runtime.
///
/// Takes the shape of a `thread_local!`, with or without a `const` initializer, and
/// expands to a `thread_local!` on native targets and an [`InstanceLocal`] static on
/// wasm ones. Call sites use `.with(|value| ...)` either way.
#[macro_export]
macro_rules! instance_local {
    ($(
        $(#[$attr:meta])*
        $vis:vis static $name:ident: $ty:ty = const $init:block;
    )*) => {$(
        #[cfg(target_family = "wasm")]
        $(#[$attr])*
        $vis static $name: $crate::instance_local::InstanceLocal<$ty> =
            $crate::instance_local::InstanceLocal::new(|| $init);

        #[cfg(not(target_family = "wasm"))]
        ::std::thread_local! {
            $(#[$attr])*
            $vis static $name: $ty = const $init;
        }
    )*};

    ($(
        $(#[$attr:meta])*
        $vis:vis static $name:ident: $ty:ty = $init:expr;
    )*) => {$(
        #[cfg(target_family = "wasm")]
        $(#[$attr])*
        $vis static $name: $crate::instance_local::InstanceLocal<$ty> =
            $crate::instance_local::InstanceLocal::new(|| $init);

        #[cfg(not(target_family = "wasm"))]
        ::std::thread_local! {
            $(#[$attr])*
            $vis static $name: $ty = $init;
        }
    )*};
}
