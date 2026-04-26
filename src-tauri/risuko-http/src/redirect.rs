/// What to do when a request needs to follow another redirect but the
/// configured budget has been exhausted
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnExceeded {
    /// Stop silently and return the last redirect response to the caller
    Stop,
    /// Surface the cap as a `Redirect` error
    Error,
}

/// Redirect policy.
///
/// `max` is the maximum number of `Location` hops to follow. `unlimited`
/// flips the policy into "never cap" mode (no `usize::MAX` sentinel needed
/// at call sites). `on_exceeded` controls whether hitting the cap produces
/// a silent stop or a `Redirect` error
#[derive(Clone, Debug)]
pub struct Policy {
    pub(crate) max: usize,
    pub(crate) unlimited: bool,
    pub(crate) on_exceeded: OnExceeded,
}

impl Policy {
    /// Follow up to `max` redirects, then error out
    pub fn limited(max: usize) -> Self {
        Self {
            max,
            unlimited: false,
            on_exceeded: OnExceeded::Error,
        }
    }

    /// Do not follow any redirects: return the 3xx response to the caller
    pub fn none() -> Self {
        Self {
            max: 0,
            unlimited: false,
            on_exceeded: OnExceeded::Stop,
        }
    }

    /// Follow redirects without a cap. Prefer this over `limited(usize::MAX)`
    /// at call sites that want "no limit" semantics
    pub fn unlimited() -> Self {
        Self {
            max: 0,
            unlimited: true,
            on_exceeded: OnExceeded::Error,
        }
    }

    /// Choose how the policy reacts when the cap is exceeded. Has no effect
    /// for [`Policy::unlimited`]
    pub fn on_exceeded(mut self, action: OnExceeded) -> Self {
        self.on_exceeded = action;
        self
    }

    pub(crate) fn follows(&self) -> bool {
        // A `limited(0)` policy still cares about redirects: it should run
        // through the redirect loop so the budget check fires immediately
        // and the configured `OnExceeded::Error` is surfaced. Only `none()`
        // (max=0, !unlimited, on_exceeded=Stop) opts out entirely
        self.unlimited || self.max > 0 || self.on_exceeded == OnExceeded::Error
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::limited(10)
    }
}
