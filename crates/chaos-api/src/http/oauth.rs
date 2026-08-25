use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Form, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use chaos_core::ApplicationError;
use chaos_domain::identity::IdentityProvider;
use secrecy::ExposeSecret as _;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::{
    http::ApiState,
    mcp::oauth::{MCP_SCOPE, McpOAuthService, OAuthClient},
};

#[rustfmt::skip]
pub(crate) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/oauth/register", post(register_client))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/authorize/consent", post(consent))
        .route("/oauth/token", post(token))
        .route("/.well-known/oauth-protected-resource", get(protected_resource_metadata))
        .route("/.well-known/oauth-protected-resource/mcp/v1", get(protected_resource_metadata))
        .route("/.well-known/oauth-authorization-server", get(authorization_server_metadata))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    client_name: String,
    redirect_uris: Vec<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    application_type: Option<String>,
    token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Serialize)]
struct RegisterResponse {
    client_id: String,
    client_id_issued_at: i64,
    client_name: String,
    redirect_uris: Vec<String>,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    token_endpoint_auth_method: String,
    application_type: String,
}

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    resource: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConsentRequest {
    transaction_id: Uuid,
    provider: String,
    identity_token: String,
}

#[derive(Debug, Serialize)]
struct ConsentResponse {
    redirect_uri: String,
}

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: u32,
    refresh_token: String,
    scope: String,
}

#[derive(Debug, Serialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    bearer_methods_supported: Vec<&'static str>,
    scopes_supported: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    response_types_supported: Vec<&'static str>,
    grant_types_supported: Vec<&'static str>,
    code_challenge_methods_supported: Vec<&'static str>,
    scopes_supported: Vec<&'static str>,
    token_endpoint_auth_methods_supported: Vec<&'static str>,
    client_id_metadata_document_supported: bool,
    authorization_response_iss_parameter_supported: bool,
}

async fn register_client(
    State(state): State<ApiState>,
    Json(body): Json<RegisterRequest>,
) -> Response {
    let Some((redirect_uris, grant_types, response_types, application_type)) =
        validate_registration(&body)
    else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "the client metadata is not supported",
        );
    };
    match state
        .mcp_oauth
        .register_client(
            body.client_name.trim().to_owned(),
            redirect_uris,
            grant_types,
            response_types,
            application_type,
        )
        .await
    {
        Ok(client) => (StatusCode::CREATED, Json(RegisterResponse::from(client))).into_response(),
        Err(error) => application_error_response(error),
    }
}

async fn authorize(State(state): State<ApiState>, Query(query): Query<AuthorizeQuery>) -> Response {
    let Some(response_type) = query.response_type.as_deref() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "response_type is required",
        );
    };
    let Some(client_id) = query.client_id.as_deref() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "client_id is required",
        );
    };
    let Some(redirect_uri) = query.redirect_uri.as_deref() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri is required",
        );
    };
    let Some(code_challenge) = query.code_challenge.as_deref() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "PKCE code_challenge is required",
        );
    };
    let Some(code_challenge_method) = query.code_challenge_method.as_deref() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "PKCE code_challenge_method is required",
        );
    };
    if response_type != "code" || code_challenge_method != "S256" {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "only response_type=code and code_challenge_method=S256 are supported",
        );
    }
    if !state.mcp_oauth.valid_resource(query.resource.as_deref()) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "resource must identify this MCP server",
        );
    }
    let scope = match McpOAuthService::normalize_scope(query.scope.as_deref()) {
        Ok(scope) => scope,
        Err(_) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_scope",
                "only the mcp scope is supported",
            );
        }
    };
    let client = match state.mcp_oauth.find_client(client_id).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_client",
                "unknown client_id",
            );
        }
        Err(error) => return application_error_response(error),
    };
    if !client
        .redirect_uris
        .iter()
        .any(|value| value == redirect_uri)
        || !client.response_types.iter().any(|value| value == "code")
        || !client
            .grant_types
            .iter()
            .any(|value| value == "authorization_code")
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the client or redirect_uri is not registered for authorization code flow",
        );
    }
    let page = match state
        .mcp_oauth
        .start_authorization(
            &client,
            redirect_uri,
            &scope,
            query.state.as_deref(),
            code_challenge,
            code_challenge_method,
        )
        .await
    {
        Ok(page) => page,
        Err(error) => return application_error_response(error),
    };
    let mut response = Html(render_authorization_page(
        &page,
        state.mcp_oauth.google_client_id(),
        state.mcp_oauth.apple_client_id(),
        state.mcp_oauth.issuer(),
    ))
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn consent(State(state): State<ApiState>, Json(body): Json<ConsentRequest>) -> Response {
    let provider = match IdentityProvider::parse(&body.provider) {
        Ok(provider) => provider,
        Err(_) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "unsupported identity provider",
            );
        }
    };
    let grant = match state
        .identity_auth
        .sign_in(provider, &secrecy::SecretString::from(body.identity_token))
        .await
    {
        Ok(grant) => grant,
        Err(_) => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "access_denied",
                "identity verification failed",
            );
        }
    };
    let user_id = match state.identity_auth.authenticate(&grant.token) {
        Ok(user_id) => user_id,
        Err(_) => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "access_denied",
                "identity verification failed",
            );
        }
    };
    match state
        .mcp_oauth
        .finish_authorization(body.transaction_id, user_id)
        .await
    {
        Ok(redirect) => Json(ConsentResponse {
            redirect_uri: redirect.location,
        })
        .into_response(),
        Err(_) => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the authorization request expired",
        ),
    }
}

async fn token(State(state): State<ApiState>, Form(body): Form<TokenRequest>) -> Response {
    let Some(client_id) = body.client_id.as_deref() else {
        return oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "client_id is required",
        );
    };
    let client = match state.mcp_oauth.find_client(client_id).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "unknown client_id",
            );
        }
        Err(error) => return application_error_response(error),
    };
    if client.token_endpoint_auth_method != "none" {
        return oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "client authentication is not supported",
        );
    }
    if !state.mcp_oauth.valid_resource(body.resource.as_deref()) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "resource must identify this MCP server",
        );
    }
    let result = match body.grant_type.as_str() {
        "authorization_code" => {
            let (Some(code), Some(redirect_uri), Some(code_verifier)) = (
                body.code.as_deref(),
                body.redirect_uri.as_deref(),
                body.code_verifier.as_deref(),
            ) else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "code, redirect_uri, and code_verifier are required",
                );
            };
            if !client
                .redirect_uris
                .iter()
                .any(|value| value == redirect_uri)
            {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "redirect_uri does not match",
                );
            }
            state
                .mcp_oauth
                .redeem_authorization_code(client_id, redirect_uri, code, code_verifier)
                .await
        }
        "refresh_token" => {
            let Some(refresh_token) = body.refresh_token.as_deref() else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "refresh_token is required",
                );
            };
            if !client
                .grant_types
                .iter()
                .any(|value| value == "refresh_token")
            {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "unauthorized_client",
                    "refresh tokens are not enabled",
                );
            }
            state
                .mcp_oauth
                .rotate_refresh_token(client_id, refresh_token, state.mcp_oauth.resource())
                .await
        }
        _ => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "unsupported_grant_type",
                "grant_type is not supported",
            );
        }
    };
    match result {
        Ok(tokens) => token_response(tokens),
        Err(ApplicationError::Unauthorized | ApplicationError::Forbidden) => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "the authorization grant is invalid or expired",
        ),
        Err(error) => application_error_response(error),
    }
}

async fn protected_resource_metadata(State(state): State<ApiState>) -> Response {
    let metadata = ProtectedResourceMetadata {
        resource: state.mcp_oauth.resource().to_owned(),
        authorization_servers: vec![state.mcp_oauth.issuer().to_owned()],
        bearer_methods_supported: vec!["header"],
        scopes_supported: vec![MCP_SCOPE],
    };
    metadata_response(Json(metadata))
}

async fn authorization_server_metadata(State(state): State<ApiState>) -> Response {
    let metadata = AuthorizationServerMetadata {
        issuer: state.mcp_oauth.issuer().to_owned(),
        authorization_endpoint: state.mcp_oauth.authorization_endpoint(),
        token_endpoint: state.mcp_oauth.token_endpoint(),
        registration_endpoint: state.mcp_oauth.registration_endpoint(),
        response_types_supported: vec!["code"],
        grant_types_supported: vec!["authorization_code", "refresh_token"],
        code_challenge_methods_supported: vec!["S256"],
        scopes_supported: vec![MCP_SCOPE],
        token_endpoint_auth_methods_supported: vec!["none"],
        client_id_metadata_document_supported: false,
        authorization_response_iss_parameter_supported: true,
    };
    metadata_response(Json(metadata))
}

type RegistrationFields = (Vec<String>, Vec<String>, Vec<String>, String);

fn validate_registration(body: &RegisterRequest) -> Option<RegistrationFields> {
    let client_name = body.client_name.trim();
    if client_name.is_empty() || client_name.len() > 120 || body.redirect_uris.is_empty() {
        return None;
    }
    let mut redirect_uris = body.redirect_uris.clone();
    redirect_uris.sort();
    redirect_uris.dedup();
    if redirect_uris.len() != body.redirect_uris.len()
        || redirect_uris.iter().any(|uri| !valid_redirect_uri(uri))
    {
        return None;
    }
    let grant_types = body
        .grant_types
        .clone()
        .unwrap_or_else(|| vec!["authorization_code".into(), "refresh_token".into()]);
    if grant_types.is_empty()
        || grant_types
            .iter()
            .any(|grant| grant != "authorization_code" && grant != "refresh_token")
        || !grant_types
            .iter()
            .any(|grant| grant == "authorization_code")
    {
        return None;
    }
    let response_types = body
        .response_types
        .clone()
        .unwrap_or_else(|| vec!["code".into()]);
    if response_types != ["code"] {
        return None;
    }
    if body
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|method| method != "none")
    {
        return None;
    }
    let application_type = body.application_type.clone().unwrap_or_else(|| {
        if redirect_uris
            .iter()
            .all(|uri| is_loopback_redirect_uri(uri))
        {
            "native".into()
        } else {
            "web".into()
        }
    });
    if application_type != "native" && application_type != "web" {
        return None;
    }
    Some((redirect_uris, grant_types, response_types, application_type))
}

fn valid_redirect_uri(value: &str) -> bool {
    let Ok(uri) = Url::parse(value) else {
        return false;
    };
    if uri.fragment().is_some() || uri.username() != "" || uri.password().is_some() {
        return false;
    }
    match uri.scheme() {
        "https" => uri.host_str().is_some(),
        "http" => is_loopback_redirect_uri(value),
        _ => false,
    }
}

fn is_loopback_redirect_uri(value: &str) -> bool {
    let Ok(uri) = Url::parse(value) else {
        return false;
    };
    matches!(uri.scheme(), "http" | "https")
        && uri
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1"))
}

fn token_response(tokens: crate::mcp::oauth::OAuthTokenSet) -> Response {
    let body = TokenResponse {
        access_token: tokens.access_token.expose_secret().to_owned(),
        token_type: "Bearer",
        expires_in: tokens.expires_in_seconds,
        refresh_token: tokens.refresh_token.expose_secret().to_owned(),
        scope: tokens.scope,
    };
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn metadata_response<T: Serialize>(body: Json<T>) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

fn application_error_response(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Unauthorized => oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_grant",
            "the authorization grant is invalid",
        ),
        ApplicationError::Forbidden => oauth_error(
            StatusCode::FORBIDDEN,
            "access_denied",
            "the operation is not allowed",
        ),
        ApplicationError::Validation { .. } => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the request is invalid",
        ),
        ApplicationError::Unavailable { service, source } => {
            tracing::warn!(%service, error = %source, "OAuth dependency unavailable");
            oauth_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "try again later",
            )
        }
        ApplicationError::Unexpected(source) => {
            tracing::error!(error = %source, "unexpected OAuth error");
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "an unexpected error occurred",
            )
        }
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the request is invalid",
        ),
    }
}

fn oauth_error(status: StatusCode, code: &str, description: &str) -> Response {
    let mut response = (
        status,
        Json(serde_json::json!({
            "error": code,
            "error_description": description,
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn render_authorization_page(
    page: &crate::mcp::oauth::AuthorizationPage,
    google_client_id: Option<&str>,
    apple_client_id: Option<&str>,
    apple_redirect_uri: &str,
) -> String {
    let config = serde_json::json!({
        "transaction_id": page.transaction_id,
        "google_client_id": google_client_id,
        "apple_client_id": apple_client_id,
        "apple_redirect_uri": apple_redirect_uri,
    })
    .to_string()
    .replace('<', "\\u003c");
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Sign in to MCP</title><style>body{font:16px system-ui,sans-serif;max-width:32rem;margin:12vh auto;padding:0 1.5rem;color:#17202a}main{border:1px solid #d9dee5;border-radius:16px;padding:2rem;box-shadow:0 8px 30px #17202a12}h1{font-size:1.35rem}#status{color:#65717f;min-height:1.5rem}.apple-button{display:block;margin-top:0.75rem;width:100%;padding:0.7rem;border:0;border-radius:8px;background:#111;color:white;font:inherit;cursor:pointer}</style></head><body><main><h1>Sign in to ",
    );
    html.push_str(&html_escape(&page.client_name));
    html.push_str("</h1><p>This MCP client requests the <code>");
    html.push_str(&html_escape(&page.scope));
    html.push_str("</code> scope.</p><div id=\"google-button\"></div><button id=\"apple-button\" class=\"apple-button\" type=\"button\" hidden>Continue with Apple</button><p id=\"status\">Choose an identity provider to continue.</p></main><script src=\"https://accounts.google.com/gsi/client\" async defer></script>");
    if apple_client_id.is_some() {
        html.push_str("<script src=\"https://appleid.cdn-apple.com/appleauth/static/jsapi/appleid/1/en_US/appleid.auth.js\" async></script>");
    }
    html.push_str("<script>const config=");
    html.push_str(&config);
    html.push_str(
        r#";
const status=document.getElementById('status');
const appleButton=document.getElementById('apple-button');
async function finish(provider,identityToken){
  status.textContent='Completing sign-in…';
  try{
    const result=await fetch('/oauth/authorize/consent',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({transaction_id:config.transaction_id,provider,identity_token:identityToken})});
    const body=await result.json();
    if(!result.ok) throw new Error(body.error_description||'Sign-in failed');
    window.location.assign(body.redirect_uri);
  }catch(error){status.textContent=error.message||'Sign-in failed';}
}
function boot(){
  if(config.google_client_id&&window.google?.accounts?.id){
    google.accounts.id.initialize({client_id:config.google_client_id,callback:response=>finish('google',response.credential)});
    google.accounts.id.renderButton(document.getElementById('google-button'),{theme:'outline',size:'large',text:'continue_with'});
  }
  if(config.apple_client_id&&window.AppleID?.auth&&appleButton){
    AppleID.auth.init({clientId:config.apple_client_id,scope:'name email',redirectURI:config.apple_redirect_uri,state:config.transaction_id,usePopup:true});
    appleButton.hidden=false;
    appleButton.addEventListener('click',async()=>{
      try{const response=await AppleID.auth.signIn(); await finish('apple',response.authorization.id_token);}
      catch(error){status.textContent=error.message||'Sign-in failed';}
    });
  }
  if(!config.google_client_id&&!config.apple_client_id) status.textContent='No browser identity provider is configured on this server.';
}
setTimeout(boot,100);setTimeout(boot,1000);</script></body></html>"#,
    );
    html
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

impl From<OAuthClient> for RegisterResponse {
    fn from(client: OAuthClient) -> Self {
        Self {
            client_id: client.client_id,
            client_id_issued_at: OffsetDateTime::now_utc().unix_timestamp(),
            client_name: client.client_name,
            redirect_uris: client.redirect_uris,
            grant_types: client.grant_types,
            response_types: client.response_types,
            token_endpoint_auth_method: client.token_endpoint_auth_method,
            application_type: client.application_type,
        }
    }
}
