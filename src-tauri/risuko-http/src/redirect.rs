/// Redirect policy: follow up to `max` `Location` hops, then surface a
/// `Redirect` error
#[derive(Clone, Debug)]
pub struct Policy {
    pub(crate) max: usize,
}

impl Policy {
    /// Follow up to `max` redirects, then error out
    pub fn limited(max: usize) -> Self {
        Self { max }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::limited(10)
    }
}
