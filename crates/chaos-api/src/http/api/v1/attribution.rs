use axum::http::{HeaderMap, header};
use chaos_core::sales::{CheckoutAttributionInput, ShopperSessionContext, UtmTags};
use serde::Deserialize;

/// Attribution captured when checkout starts and retained for the later
/// server-side Purchase event.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CheckoutAttributionRequest {
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    utm: Option<UtmAttributionRequest>,
    #[serde(default)]
    meta: Option<MetaAttributionRequest>,
}

/// Acquisition context captured when a shopper session is created or touched.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionAttributionRequest {
    #[serde(default)]
    utm: Option<UtmAttributionRequest>,
}

/// Standard `utm_*` campaign tags without the redundant prefix because they
/// are already namespaced under `attribution.utm`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UtmAttributionRequest {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    campaign: Option<String>,
    #[serde(default)]
    term: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaAttributionRequest {
    #[serde(default)]
    fbc: Option<String>,
    #[serde(default)]
    fbp: Option<String>,
}

pub(super) fn checkout_attribution_input(
    attribution: Option<&CheckoutAttributionRequest>,
    headers: &HeaderMap,
) -> Option<CheckoutAttributionInput> {
    let meta = attribution.and_then(|value| value.meta.as_ref());
    Some(CheckoutAttributionInput {
        meta_fbc: meta.and_then(|value| value.fbc.clone()),
        meta_fbp: meta.and_then(|value| value.fbp.clone()),
        client_ip_address: client_ip_address(headers),
        client_user_agent: client_user_agent(headers),
        source_url: attribution.and_then(|value| value.source_url.clone()),
        utm: utm_tags(attribution.and_then(|value| value.utm.as_ref())),
    })
}

pub(super) fn shopper_session_context(
    attribution: Option<&SessionAttributionRequest>,
    headers: &HeaderMap,
) -> ShopperSessionContext {
    ShopperSessionContext {
        user_agent: client_user_agent(headers),
        ip_address: client_ip_address(headers),
        utm: utm_tags(attribution.and_then(|value| value.utm.as_ref())),
    }
}

fn client_ip_address(headers: &HeaderMap) -> Option<String> {
    // Nginx writes one canonical client address into X-Real-IP. The browser
    // cannot provide this field through the attribution body.
    headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<std::net::IpAddr>().ok())
        .map(|value| value.to_string())
}

fn client_user_agent(headers: &HeaderMap) -> Option<String> {
    // User-Agent belongs to the request that captured the attribution rather
    // than to the browser-controlled JSON body.
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn utm_tags(value: Option<&UtmAttributionRequest>) -> UtmTags {
    UtmTags {
        source: value.and_then(|value| value.source.clone()),
        medium: value.and_then(|value| value.medium.clone()),
        campaign: value.and_then(|value| value.campaign.clone()),
        term: value.and_then(|value| value.term.clone()),
        content: value.and_then(|value| value.content.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_context_accepts_v4_and_v6_but_omits_invalid_ip() {
        for (raw, expected) in [
            ("203.0.113.10", Some("203.0.113.10")),
            ("2001:db8::1", Some("2001:db8::1")),
            ("not-an-ip", None),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("x-real-ip", raw.parse().unwrap());

            let context = shopper_session_context(None, &headers);

            assert_eq!(context.ip_address.as_deref(), expected);
        }
    }
}
