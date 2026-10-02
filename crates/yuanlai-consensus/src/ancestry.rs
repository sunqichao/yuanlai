use yuanlai_types::{Height, Timestamp};

/// Read access to the branch ending at a particular block.
///
/// Implementations must answer for the branch being validated, not for the current best chain.
pub trait Ancestry {
    /// Timestamp of the block at `height` on this branch, or `None` if the branch is shorter.
    fn timestamp_at(&self, height: Height) -> Option<Timestamp>;
}
