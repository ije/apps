pub use std::time::Instant;

pub trait Clock {
    fn now(&self) -> Instant;
}
