use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::Mutex;

use crate::config::RateLimitConfig;

#[derive(Debug)]
pub struct RateLimiter {
    permits: u64,
    interval: Duration,
    state: Mutex<TokenBucketState>,
}

#[derive(Debug)]
struct TokenBucketState {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(permits: u64, interval: Duration) -> Result<Self> {
        if permits == 0 {
            bail!("rate limiter permits must be greater than 0");
        }

        if interval.is_zero() {
            bail!("rate limiter interval must be greater than 0");
        }

        Ok(Self {
            permits,
            interval,
            state: Mutex::new(TokenBucketState {
                tokens: 1.0,
                last_refill: Instant::now(),
            }),
        })
    }

    pub fn from_config(config: RateLimitConfig) -> Result<Self> {
        Self::new(config.rate, config.interval())
    }

    pub fn permits(&self) -> u64 {
        self.permits
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub async fn acquire(&self) {
        loop {
            let wait_duration = {
                let mut state = self.state.lock().await;

                self.refill(&mut state);

                if state.tokens >= 1.0 {
                    state.tokens -= 1.0;
                    return;
                }

                let missing_tokens = 1.0 - state.tokens;
                self.duration_for_tokens(missing_tokens)
            };

            tokio::time::sleep(wait_duration).await;
        }
    }

    fn refill(&self, state: &mut TokenBucketState) {
        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill);

        if elapsed.is_zero() {
            return;
        }

        let refill_tokens =
            elapsed.as_secs_f64() * self.permits as f64 / self.interval.as_secs_f64();

        state.tokens = (state.tokens + refill_tokens).min(self.permits as f64);
        state.last_refill = now;
    }

    fn duration_for_tokens(&self, tokens: f64) -> Duration {
        Duration::from_secs_f64(tokens * self.interval.as_secs_f64() / self.permits as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{RateLimitConfig, RatePeriod};

    #[test]
    fn new_rejects_zero_permits() {
        assert!(RateLimiter::new(0, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn new_rejects_zero_interval() {
        assert!(RateLimiter::new(1, Duration::ZERO).is_err());
    }

    #[test]
    fn from_config_uses_rate_and_period() -> Result<()> {
        let limiter = RateLimiter::from_config(RateLimitConfig {
            rate: 5000,
            period: RatePeriod::Minute,
        })?;

        assert_eq!(limiter.permits(), 5000);
        assert_eq!(limiter.interval(), Duration::from_secs(60));

        Ok(())
    }

    #[tokio::test]
    async fn acquire_throttles_when_tokens_are_exhausted() -> Result<()> {
        let limiter = RateLimiter::new(5, Duration::from_secs(1))?;
        let started_at = Instant::now();

        for _ in 0..6 {
            limiter.acquire().await;
        }

        assert!(started_at.elapsed() >= Duration::from_millis(900));

        Ok(())
    }

    #[tokio::test]
    async fn acquire_supports_high_rates() -> Result<()> {
        let limiter = RateLimiter::new(100_000, Duration::from_secs(1))?;

        for _ in 0..10 {
            limiter.acquire().await;
        }

        Ok(())
    }

    #[tokio::test]
    async fn acquire_stays_within_expected_rate_window() -> Result<()> {
        let limiter = RateLimiter::new(10, Duration::from_secs(1))?;
        let started_at = Instant::now();

        for _ in 0..11 {
            limiter.acquire().await;
        }

        let elapsed = started_at.elapsed();

        assert!(
            elapsed >= Duration::from_millis(900),
            "expected at least 900ms for 11 acquires at 10/s, got {:?}",
            elapsed
        );

        assert!(
            elapsed <= Duration::from_millis(1_500),
            "expected no more than 1500ms for 11 acquires at 10/s, got {:?}",
            elapsed
        );

        Ok(())
    }
}
