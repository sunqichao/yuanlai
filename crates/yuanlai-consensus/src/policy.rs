//! Non-consensus acceptance policy. These checks may depend on local state such as the clock and
//! must never mark a block permanently invalid.

use yuanlai_types::params::MAX_FUTURE_DRIFT;
use yuanlai_types::{BlockHeader, Timestamp};

/// BLK-ACCEPT-01: whether acceptance of `header` should be deferred because its timestamp is more
/// than `MAX_FUTURE_DRIFT` seconds ahead of `local_time`.
#[must_use]
pub fn is_too_far_in_future(header: &BlockHeader, local_time: Timestamp) -> bool {
    header.timestamp.0 > local_time.0.saturating_add(MAX_FUTURE_DRIFT)
}
