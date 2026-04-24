use governor::{Quota, RateLimiter};
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use std::num::NonZeroU32;
use std::sync::Arc;

pub type ApiLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// e621's documented API rate limit is 2 requests per second, per app.
pub const API_RPS: u32 = 2;

pub fn new_api_limiter() -> Arc<ApiLimiter> {
    let quota = Quota::per_second(NonZeroU32::new(API_RPS).expect("non-zero"));
    Arc::new(RateLimiter::direct(quota))
}
