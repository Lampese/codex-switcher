//! Local OAuth server for handling ChatGPT login flow

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tiny_http::{Header, Request, Response, Server};
use tokio::sync::oneshot;

use crate::types::{OAuthLoginInfo, StoredAccount};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_PORT: u16 = 1455; // Same as official Codex

/// PKCE codes for OAuth
#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generate PKCE codes
pub fn generate_pkce() -> PkceCodes {
    let mut bytes = [0u8; 64];
    rand::rng().fill_bytes(&mut bytes);

    let code_verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);

    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

/// Generate a random state parameter
fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the OAuth authorization URL
fn build_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", &pkce.code_challenge),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", "codex_cli_rs"), // Required by OpenAI OAuth
    ];

    let query_string = params
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    format!("{issuer}/oauth/authorize?{query_string}")
}

/// Token response from the OAuth server
#[derive(Debug, Clone, serde::Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

/// Exchange authorization code for tokens
async fn exchange_code_for_tokens(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> Result<TokenResponse> {
    let client = reqwest::Client::new();

    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencoding::encode(code),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(client_id),
        urlencoding::encode(&pkce.code_verifier)
    );

    let resp = client
        .post(format!("{issuer}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("Failed to send token request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed: {status} - {body}");
    }

    let tokens: TokenResponse = resp
        .json()
        .await
        .context("Failed to parse token response")?;
    Ok(tokens)
}

/// Parse claims from JWT ID token
fn parse_id_token_claims(id_token: &str) -> (Option<String>, Option<String>, Option<String>) {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return (None, None, None);
    }

    let payload = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(bytes) => bytes,
        Err(_) => return (None, None, None),
    };

    let json: serde_json::Value = match serde_json::from_slice(&payload) {
        Ok(v) => v,
        Err(_) => return (None, None, None),
    };

    let email = json.get("email").and_then(|v| v.as_str()).map(String::from);

    let auth_claims = json.get("https://api.openai.com/auth");

    let plan_type = auth_claims
        .and_then(|auth| auth.get("chatgpt_plan_type"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let account_id = auth_claims
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    (email, plan_type, account_id)
}

/// OAuth login flow result
pub struct OAuthLoginResult {
    pub account: StoredAccount,
}

/// Start the OAuth login flow
pub async fn start_oauth_login(
    account_name: String,
) -> Result<(OAuthLoginInfo, oneshot::Receiver<Result<OAuthLoginResult>>)> {
    let pkce = generate_pkce();
    let state = generate_state();

    println!("[OAuth] Starting login for account: {account_name}");
    println!("[OAuth] PKCE challenge: {}", &pkce.code_challenge[..20]);

    // Try to bind to the server
    let server = Server::http(format!("127.0.0.1:{DEFAULT_PORT}"))
        .map_err(|e| anyhow::anyhow!("Failed to start OAuth server: {e}"))?;

    let actual_port = match server.server_addr().to_ip() {
        Some(addr) => addr.port(),
        None => anyhow::bail!("Failed to determine server port"),
    };

    let redirect_uri = format!("http://localhost:{actual_port}/auth/callback");
    let auth_url = build_authorize_url(DEFAULT_ISSUER, CLIENT_ID, &redirect_uri, &pkce, &state);

    println!("[OAuth] Server started on port {actual_port}");
    println!("[OAuth] Redirect URI: {redirect_uri}");
    println!("[OAuth] Auth URL: {auth_url}");

    let login_info = OAuthLoginInfo {
        auth_url: auth_url.clone(),
        callback_port: actual_port,
    };

    // Create a channel for the result
    let (tx, rx) = oneshot::channel();

    // Spawn the server in a background thread
    let server = Arc::new(server);
    let pkce_clone = pkce.clone();
    let state_clone = state.clone();

    thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(run_oauth_server(
            server,
            pkce_clone,
            state_clone,
            redirect_uri,
            account_name,
        ));
        let _ = tx.send(result);
    });

    // Open the browser
    let _ = webbrowser::open(&auth_url);

    Ok((login_info, rx))
}

/// Run the OAuth callback server
async fn run_oauth_server(
    server: Arc<Server>,
    pkce: PkceCodes,
    expected_state: String,
    redirect_uri: String,
    account_name: String,
) -> Result<OAuthLoginResult> {
    let timeout = Duration::from_secs(300); // 5 minute timeout
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!("OAuth login timed out");
        }

        // Use recv_timeout to allow checking the timeout
        let request = match server.recv_timeout(Duration::from_secs(1)) {
            Ok(Some(req)) => req,
            Ok(None) => continue,
            Err(_) => continue,
        };

        let result = handle_oauth_request(
            request,
            &pkce,
            &expected_state,
            &redirect_uri,
            &account_name,
        )
        .await;

        match result {
            HandleResult::Continue => continue,
            HandleResult::Success(account) => {
                server.unblock();
                return Ok(OAuthLoginResult { account });
            }
            HandleResult::Error(e) => {
                server.unblock();
                return Err(e);
            }
        }
    }
}

enum HandleResult {
    Continue,
    Success(StoredAccount),
    Error(anyhow::Error),
}

async fn handle_oauth_request(
    request: Request,
    pkce: &PkceCodes,
    expected_state: &str,
    redirect_uri: &str,
    account_name: &str,
) -> HandleResult {
    let url_str = request.url().to_string();
    let parsed = match url::Url::parse(&format!("http://localhost{url_str}")) {
        Ok(u) => u,
        Err(_) => {
            let _ = request.respond(Response::from_string("Bad Request").with_status_code(400));
            return HandleResult::Continue;
        }
    };

    let path = parsed.path();

    if path == "/auth/callback" {
        println!("[OAuth] Received callback request");
        let params: std::collections::HashMap<String, String> =
            parsed.query_pairs().into_owned().collect();

        println!(
            "[OAuth] Callback params: {:?}",
            params.keys().collect::<Vec<_>>()
        );

        // Check for error response
        if let Some(error) = params.get("error") {
            let error_desc = params
                .get("error_description")
                .map(|s| s.as_str())
                .unwrap_or("Unknown error");
            println!("[OAuth] Error from provider: {error} - {error_desc}");
            let _ = request.respond(
                Response::from_string(format!("OAuth Error: {error} - {error_desc}"))
                    .with_status_code(400),
            );
            return HandleResult::Error(anyhow::anyhow!("OAuth error: {error} - {error_desc}"));
        }

        // Verify state
        if params.get("state").map(String::as_str) != Some(expected_state) {
            println!("[OAuth] State mismatch!");
            let _ = request.respond(Response::from_string("State mismatch").with_status_code(400));
            return HandleResult::Error(anyhow::anyhow!("OAuth state mismatch"));
        }

        println!("[OAuth] State verified OK");

        // Get the authorization code
        let code = match params.get("code") {
            Some(c) if !c.is_empty() => c.clone(),
            _ => {
                println!("[OAuth] Missing authorization code");
                let _ = request.respond(
                    Response::from_string("Missing authorization code").with_status_code(400),
                );
                return HandleResult::Error(anyhow::anyhow!("Missing authorization code"));
            }
        };

        println!("[OAuth] Got authorization code, exchanging for tokens...");

        // Exchange code for tokens
        match exchange_code_for_tokens(DEFAULT_ISSUER, CLIENT_ID, redirect_uri, pkce, &code).await {
            Ok(tokens) => {
                println!("[OAuth] Token exchange successful!");
                // Parse claims from ID token
                let (email, plan_type, chatgpt_account_id) =
                    parse_id_token_claims(&tokens.id_token);

                // Create the account
                let account = StoredAccount::new_chatgpt(
                    account_name.to_string(),
                    email,
                    plan_type,
                    tokens.id_token,
                    tokens.access_token,
                    tokens.refresh_token,
                    chatgpt_account_id,
                );

                // Send success response
                let success_html = r#"<!DOCTYPE html>
<html>
<head>
    <title>Login Successful</title>
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; display: flex; justify-content: center; align-items: center; height: 100vh; margin: 0; background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); }
        .container { text-align: center; background: white; padding: 40px 60px; border-radius: 16px; box-shadow: 0 20px 60px rgba(0,0,0,0.3); }
        h1 { color: #333; margin-bottom: 10px; }
        p { color: #666; }
        .checkmark { font-size: 48px; margin-bottom: 20px; }
    </style>
</head>
<body>
    <div class="container">
        <div class="checkmark">✓</div>
        <h1>Login Successful!</h1>
        <p>You can close this window and return to Codex Switcher.</p>
    </div>
</body>
</html>"#;

                let response = Response::from_string(success_html).with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
                        .unwrap(),
                );
                let _ = request.respond(response);

                return HandleResult::Success(account);
            }
            Err(e) => {
                println!("[OAuth] Token exchange failed: {e}");
                let _ = request.respond(
                    Response::from_string(format!("Token exchange failed: {e}"))
                        .with_status_code(500),
                );
                return HandleResult::Error(e);
            }
        }
    }

    // Handle other paths
    let _ = request.respond(Response::from_string("Not Found").with_status_code(404));
    HandleResult::Continue
}

/// Wait for the OAuth login to complete
pub async fn wait_for_oauth_login(
    rx: oneshot::Receiver<Result<OAuthLoginResult>>,
) -> Result<StoredAccount> {
    let result = rx.await.context("OAuth login was cancelled")??;
    Ok(result.account)
}
