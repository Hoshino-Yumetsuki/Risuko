/// Redirect policy
#[derive(Clone, Debug)]
pub struct Policy {
    pub(crate) max: usize,
}

impl Policy {
    pub fn limited(max: usize) -> Self {
        Self { max }
    }

    pub fn none() -> Self {
        Self { max: 0 }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::limited(10)
    }
}
