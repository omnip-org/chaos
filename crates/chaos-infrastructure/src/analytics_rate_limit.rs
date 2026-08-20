use async_trait::async_trait;
use chaos_application::{
    ApplicationError,
    ports::{AnalyticsCollectionRateLimiter, AnalyticsRateLimitDecision},
};
use chaos_domain::store::{SalesChannelId, StoreId};
use redis::{Client as RedisClient, Script};
use uuid::Uuid;

const WINDOW_SECONDS: u32 = 60;
const STORE_CHANNEL_EVENT_LIMIT: u16 = 1_200;
const VISITOR_EVENT_LIMIT: u16 = 120;

pub struct RedisAnalyticsCollectionRateLimiter {
    redis: RedisClient,
}

impl RedisAnalyticsCollectionRateLimiter {
    pub const fn new(redis: RedisClient) -> Self {
        Self { redis }
    }
}

#[async_trait]
impl AnalyticsCollectionRateLimiter for RedisAnalyticsCollectionRateLimiter {
    async fn consume(
        &self,
        store_id: StoreId,
        sales_channel_id: SalesChannelId,
        visitor_event_counts: &[(Uuid, u16)],
        event_count: u16,
    ) -> Result<AnalyticsRateLimitDecision, ApplicationError> {
        let slot = format!("{}:{}", store_id.as_uuid(), sales_channel_id.as_uuid());
        let script = Script::new(
            "local retry_after = 0\n\
             for index, key in ipairs(KEYS) do\n\
               local cost = tonumber(ARGV[index * 2])\n\
               local limit = tonumber(ARGV[index * 2 + 1])\n\
               local current = tonumber(redis.call('GET', key) or '0')\n\
               if current + cost > limit then\n\
                 local ttl = redis.call('TTL', key)\n\
                 if ttl < 1 then ttl = tonumber(ARGV[1]) end\n\
                 if ttl > retry_after then retry_after = ttl end\n\
               end\n\
             end\n\
             if retry_after > 0 then return {0, retry_after} end\n\
             for index, key in ipairs(KEYS) do\n\
               local count = redis.call('INCRBY', key, ARGV[index * 2])\n\
               if count == tonumber(ARGV[index * 2]) then\n\
                 redis.call('EXPIRE', key, ARGV[1])\n\
               end\n\
             end\n\
             return {1, tonumber(ARGV[1])}",
        );
        let mut invocation = script.prepare_invoke();
        invocation
            .key(format!("chaos:analytics:rate:{{{slot}}}:all"))
            .arg(WINDOW_SECONDS)
            .arg(event_count)
            .arg(STORE_CHANNEL_EVENT_LIMIT);
        for (visitor_id, count) in visitor_event_counts {
            invocation
                .key(format!(
                    "chaos:analytics:rate:{{{slot}}}:visitor:{visitor_id}"
                ))
                .arg(*count)
                .arg(VISITOR_EVENT_LIMIT);
        }
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(redis_error)?;
        let (allowed, retry_after_seconds): (i64, i64) = invocation
            .invoke_async(&mut connection)
            .await
            .map_err(redis_error)?;
        Ok(AnalyticsRateLimitDecision {
            allowed: allowed == 1,
            retry_after_seconds: u32::try_from(retry_after_seconds.max(1))
                .unwrap_or(WINDOW_SECONDS),
        })
    }
}

fn redis_error(error: redis::RedisError) -> ApplicationError {
    ApplicationError::Unavailable {
        service: "redis",
        source: error.into(),
    }
}

#[cfg(test)]
mod tests {
    use chaos_domain::store::{SalesChannelId, StoreId};

    use super::*;

    #[tokio::test]
    #[ignore = "requires TEST_REDIS_URL with a disposable Redis database"]
    async fn atomically_limits_visitor_and_store_channel_event_volume() {
        let redis_url = std::env::var("TEST_REDIS_URL")
            .expect("TEST_REDIS_URL must identify a disposable Redis database");
        let client = RedisClient::open(redis_url).unwrap();
        let limiter = RedisAnalyticsCollectionRateLimiter::new(client);
        let store_id = StoreId::new();
        let channel_id = SalesChannelId::new();
        let visitor_id = Uuid::now_v7();

        let allowed = limiter
            .consume(
                store_id,
                channel_id,
                &[(visitor_id, VISITOR_EVENT_LIMIT)],
                VISITOR_EVENT_LIMIT,
            )
            .await
            .unwrap();
        assert!(allowed.allowed);

        let denied = limiter
            .consume(store_id, channel_id, &[(visitor_id, 1)], 1)
            .await
            .unwrap();
        assert!(!denied.allowed);
        assert!((1..=WINDOW_SECONDS).contains(&denied.retry_after_seconds));
    }
}
