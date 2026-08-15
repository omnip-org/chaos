use std::time::Instant;

use axum::{
    Router,
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};

use super::ApiState;

pub fn routes() -> Router<ApiState> {
    Router::new().route("/", get(scrape))
}

async fn scrape(State(state): State<ApiState>) -> impl IntoResponse {
    let pool = state.infrastructure.runtime_pool();
    ::metrics::gauge!("chaos_database_pool_size").set(pool.size() as f64);
    ::metrics::gauge!("chaos_database_pool_idle").set(pool.num_idle() as f64);
    let dependency_healthy = tokio::time::timeout(
        state.infrastructure.dependency_timeout,
        state.infrastructure.check_dependencies(),
    )
    .await
    .is_ok_and(|result| result.is_ok());
    ::metrics::gauge!("chaos_dependencies_healthy").set(f64::from(dependency_healthy));
    if let Ok((pending, dead_letter, oldest)) =
        sqlx::query_as::<_, (i64, i64, f64)>("SELECT * FROM integration.queue_metrics()")
            .fetch_one(&pool)
            .await
    {
        ::metrics::gauge!("chaos_outbox_pending").set(pending as f64);
        ::metrics::gauge!("chaos_outbox_dead_letter").set(dead_letter as f64);
        ::metrics::gauge!("chaos_outbox_oldest_pending_seconds").set(oldest);
    }
    if let Ok((due, retrying, expiring, action_required)) =
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT * FROM payments.provider_readiness_metrics()",
        )
        .fetch_one(&pool)
        .await
    {
        ::metrics::gauge!("chaos_payment_provider_readiness_due").set(due as f64);
        ::metrics::gauge!("chaos_payment_provider_readiness_retrying").set(retrying as f64);
        ::metrics::gauge!("chaos_payment_provider_readiness_expiring").set(expiring as f64);
        ::metrics::gauge!("chaos_payment_provider_action_required").set(action_required as f64);
    }
    if let Ok((pending, processing, dead_letter, suppressed, oldest)) =
        sqlx::query_as::<_, (i64, i64, i64, i64, f64)>(
            "SELECT * FROM notification.email_delivery_metrics()",
        )
        .fetch_one(&pool)
        .await
    {
        ::metrics::gauge!("chaos_notification_email_pending").set(pending as f64);
        ::metrics::gauge!("chaos_notification_email_processing").set(processing as f64);
        ::metrics::gauge!("chaos_notification_email_dead_letter").set(dead_letter as f64);
        ::metrics::gauge!("chaos_notification_email_suppressed").set(suppressed as f64);
        ::metrics::gauge!("chaos_notification_email_oldest_pending_seconds").set(oldest);
    }
    if let Ok((due, processing, dead_letter, oldest)) = sqlx::query_as::<_, (i64, i64, i64, f64)>(
        "SELECT * FROM fulfillment.shipping_tracking_metrics()",
    )
    .fetch_one(&pool)
    .await
    {
        ::metrics::gauge!("chaos_shipping_tracking_due").set(due as f64);
        ::metrics::gauge!("chaos_shipping_tracking_processing").set(processing as f64);
        ::metrics::gauge!("chaos_shipping_tracking_dead_letter").set(dead_letter as f64);
        ::metrics::gauge!("chaos_shipping_tracking_oldest_due_seconds").set(oldest);
    }
    if let Ok((due, processing, dead_letter, oldest)) = sqlx::query_as::<_, (i64, i64, i64, f64)>(
        "SELECT * FROM fulfillment.shipping_cancellation_metrics()",
    )
    .fetch_one(&pool)
    .await
    {
        ::metrics::gauge!("chaos_shipping_cancellation_due").set(due as f64);
        ::metrics::gauge!("chaos_shipping_cancellation_processing").set(processing as f64);
        ::metrics::gauge!("chaos_shipping_cancellation_dead_letter").set(dead_letter as f64);
        ::metrics::gauge!("chaos_shipping_cancellation_oldest_due_seconds").set(oldest);
    }
    if let Ok((pending, processing, dead_letter, oldest)) =
        sqlx::query_as::<_, (i64, i64, i64, f64)>(
            "SELECT * FROM analytics.sessionization_metrics()",
        )
        .fetch_one(&pool)
        .await
    {
        ::metrics::gauge!("chaos_analytics_sessionization_pending").set(pending as f64);
        ::metrics::gauge!("chaos_analytics_sessionization_processing").set(processing as f64);
        ::metrics::gauge!("chaos_analytics_sessionization_dead_letter").set(dead_letter as f64);
        ::metrics::gauge!("chaos_analytics_sessionization_oldest_pending_seconds").set(oldest);
    }
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        state.metrics.render(),
    )
}

pub async fn track_http_request(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let started_at = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();

    ::metrics::counter!(
        "chaos_http_requests_total",
        "method" => method.clone(),
        "route" => route.clone(),
        "status" => status
    )
    .increment(1);
    ::metrics::histogram!(
        "chaos_http_request_duration_seconds",
        "method" => method,
        "route" => route.clone()
    )
    .record(started_at.elapsed().as_secs_f64());
    if route.ends_with("/order") && response.status().is_success() {
        ::metrics::counter!("chaos_checkout_conversions_total").increment(1);
    }
    if route.contains("payment") && response.status().is_server_error() {
        ::metrics::counter!("chaos_payment_failures_total").increment(1);
    }
    if route.contains("checkout") && response.status().as_u16() == 409 {
        ::metrics::counter!("chaos_inventory_reservation_conflicts_total").increment(1);
    }

    response
}
