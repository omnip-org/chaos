use crate::ports::Clock;
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        let now = OffsetDateTime::now_utc();
        now.replace_nanosecond(now.nanosecond() / 1_000 * 1_000)
            .expect("a truncated nanosecond value is always valid")
    }
}

#[cfg(test)]
mod tests {
    use crate::ports::Clock;

    use super::SystemClock;

    #[test]
    fn system_clock_matches_postgresql_microsecond_precision() {
        assert_eq!(SystemClock.now().nanosecond() % 1_000, 0);
    }
}
