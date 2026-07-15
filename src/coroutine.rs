//! Generator-shape coroutine driver mirroring `core::ops::Coroutine`:
//! `Yield` associated type for intermediate progress, `Return` for
//! terminal output, and a two-variant [`HttpCoroutineState`]
//! (`Yielded` / `Complete`).

use alloc::vec::Vec;

/// State yielded by an [`HttpCoroutine::resume`] step.
#[derive(Debug)]
pub enum HttpCoroutineState<Y, R> {
    /// Intermediate step: the coroutine needs the caller to perform
    /// the carried I/O request before the next resume.
    Yielded(Y),
    /// Terminal step: the coroutine is done and carries its final
    /// output or error.
    Complete(R),
}

/// Standard-shape HTTP coroutine: own internal state, declare per-step
/// `Yield`, return `Result<Output, Error>` on completion.
pub trait HttpCoroutine {
    /// The intermediate value emitted on every yielded step.
    type Yield;
    /// The terminal value emitted on completion.
    type Return;

    /// Advances one step. Pass [`None`] initially or after [`HttpYield::WantsWrite`];
    /// pass `Some(data)` after [`HttpYield::WantsRead`]; pass `Some(&[])` for EOF.
    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return>;
}

/// Standard I/O-only Yield; pick `type Yield = HttpYield` when the
/// coroutine only reads or writes socket bytes.
#[derive(Debug)]
pub enum HttpYield {
    /// The coroutine wants bytes read from the stream and handed back
    /// on the next resume.
    WantsRead,
    /// The coroutine wants these bytes written to the stream.
    WantsWrite(Vec<u8>),
}

/// Coroutine `?`: forwards `Yielded` (via `Into`), short-circuits on
/// `Err` (via `Into`), evaluates to the inner `Ok` value.
#[macro_export]
macro_rules! http_try {
    ($coroutine:expr, $arg:expr $(,)?) => {
        match $crate::coroutine::HttpCoroutine::resume($coroutine, $arg) {
            $crate::coroutine::HttpCoroutineState::Yielded(y) => {
                return $crate::coroutine::HttpCoroutineState::Yielded(y.into());
            }
            $crate::coroutine::HttpCoroutineState::Complete(Err(err)) => {
                return $crate::coroutine::HttpCoroutineState::Complete(Err(err.into()));
            }
            $crate::coroutine::HttpCoroutineState::Complete(Ok(value)) => value,
        }
    };
}
