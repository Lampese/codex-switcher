use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use rand::RngCore;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use tokio::runtime::Runtime;

use crate::commands::{
    add_account_from_auth_json_text, add_account_from_file, cancel_login, check_codex_processes,
    complete_login, delete_account, export_accounts_full_encrypted_bytes,
    export_accounts_slim_text, get_active_account_info, get_masked_account_ids, get_usage,
    import_accounts_full_encrypted_bytes, import_accounts_slim_text, kill_codex_processes,
    list_accounts, refresh_account_metadata, refresh_all_accounts_usage, rename_account,
    set_masked_account_ids, start_login, switch_account, warmup_account, warmup_all_accounts,
};

pub(crate) const WEB_TOKEN_HEADER: &str = "x-codex-switcher-token";
const WEB_TOKEN_COOKIE: &str = "codex_switcher_web_token";
const WEB_TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone)]
pub struct WebSecurity {
    token: Option<String>,
}

impl WebSecurity {
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountIdArgs {
    #[serde(alias = "account_id")]
    account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameAccountArgs {
    #[serde(alias = "account_id")]
    account_id: String,
    #[serde(alias = "new_name")]
    new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginArgs {
    #[serde(alias = "account_name")]
    account_name: String,
}

#[derive(Debug, Deserialize)]
struct ImportSlimArgs {
    payload: String,
}

#[derive(Debug, Deserialize)]
struct MaskedIdsArgs {
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UploadAuthJsonArgs {
    name: String,
    contents: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadEncryptedArgs {
    #[serde(alias = "contents_base64")]
    contents_base64: String,
    passphrase: String,
}

#[derive(Debug, Deserialize)]
struct ExportEncryptedArgs {
    passphrase: String,
}

#[derive(Debug, Deserialize)]
struct FileImportArgs {
    path: String,
    name: String,
}

pub fn default_web_host() -> String {
    std::env::var("CODEX_SWITCHER_WEB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
}

pub fn web_security_for_host(
    host: &str,
    configured_token: Option<String>,
) -> anyhow::Result<WebSecurity> {
    if is_loopback_host(host) {
        return Ok(WebSecurity { token: None });
    }

    let token = match configured_token {
        Some(token) if !token.trim().is_empty() => token.trim().to_string(),
        _ => generate_web_token(),
    };

    Ok(WebSecurity { token: Some(token) })
}

pub fn run_lan_server(host: &str, port: u16) -> anyhow::Result<()> {
    let security = web_security_for_host(host, std::env::var("CODEX_SWITCHER_WEB_TOKEN").ok())?;
    let address = format!("{host}:{port}");
    let server = Server::http(&address)
        .map_err(|err| anyhow::anyhow!("Failed to bind HTTP server on {address}: {err}"))?;
    let runtime = Runtime::new().context("Failed to start async runtime")?;
    let dist_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("dist");

    println!("Codex Switcher web server listening on http://{address}");
    println!("Serving static files from {}", dist_dir.display());
    if let Some(token) = security.token() {
        println!("Web access token required. Open http://{address}/?token={token}");
    }

    for request in server.incoming_requests() {
        if let Err(error) = handle_request(request, &runtime, &dist_dir, &security) {
            eprintln!("[web] request failed: {error:#}");
        }
    }

    Ok(())
}

fn handle_request(
    mut request: Request,
    runtime: &Runtime,
    dist_dir: &Path,
    security: &WebSecurity,
) -> anyhow::Result<()> {
    let method = request.method().clone();
    let url = request.url().to_string();
    let headers = request_headers(&request);

    if method == Method::Get && url == "/api/health" {
        respond_json(request, StatusCode(200), &json!({ "ok": true }))?;
        return Ok(());
    }

    if !request_is_authorized(&url, &headers, security) {
        respond_text(
            request,
            StatusCode(401),
            "Unauthorized",
            "text/plain; charset=utf-8",
            None,
        )?;
        return Ok(());
    }

    if method == Method::Post && url.starts_with("/api/invoke/") {
        let command = url.trim_start_matches("/api/invoke/");
        let payload = parse_request_json(&mut request)?;
        let result = runtime.block_on(invoke_web_command(command, payload));
        match result {
            Ok(value) => respond_json(request, StatusCode(200), &value)?,
            Err(error) => respond_json(request, StatusCode(400), &json!({ "error": error }))?,
        }
        return Ok(());
    }

    if method == Method::Get {
        serve_static(request, dist_dir, &url, security)?;
        return Ok(());
    }

    respond_text(
        request,
        StatusCode(405),
        "Method Not Allowed",
        "text/plain; charset=utf-8",
        None,
    )?;
    Ok(())
}

async fn invoke_web_command(command: &str, payload: Value) -> Result<Value, String> {
    match command {
        "list_accounts" => to_json(list_accounts().await?),
        "get_active_account_info" => to_json(get_active_account_info().await?),
        "add_account_from_file" => {
            let args: FileImportArgs = parse_args(payload)?;
            to_json(add_account_from_file(args.path, args.name).await?)
        }
        "add_account_from_auth_json_text" => {
            let args: UploadAuthJsonArgs = parse_args(payload)?;
            to_json(add_account_from_auth_json_text(args.name, args.contents).await?)
        }
        "get_usage" => {
            let args: AccountIdArgs = parse_args(payload)?;
            to_json(get_usage(args.account_id).await?)
        }
        "refresh_account_metadata" => {
            let args: AccountIdArgs = parse_args(payload)?;
            to_json(refresh_account_metadata(args.account_id).await?)
        }
        "refresh_all_accounts_usage" => to_json(refresh_all_accounts_usage().await?),
        "warmup_account" => {
            let args: AccountIdArgs = parse_args(payload)?;
            to_json(warmup_account(args.account_id).await?)
        }
        "warmup_all_accounts" => to_json(warmup_all_accounts().await?),
        "switch_account" => {
            let args: AccountIdArgs = parse_args(payload)?;
            to_json(switch_account(args.account_id).await?)
        }
        "delete_account" => {
            let args: AccountIdArgs = parse_args(payload)?;
            to_json(delete_account(args.account_id).await?)
        }
        "rename_account" => {
            let args: RenameAccountArgs = parse_args(payload)?;
            to_json(rename_account(args.account_id, args.new_name).await?)
        }
        "start_login" => {
            let args: LoginArgs = parse_args(payload)?;
            to_json(start_login(args.account_name).await?)
        }
        "complete_login" => to_json(complete_login().await?),
        "cancel_login" => to_json(cancel_login().await?),
        "export_accounts_slim_text" => to_json(export_accounts_slim_text().await?),
        "import_accounts_slim_text" => {
            let args: ImportSlimArgs = parse_args(payload)?;
            to_json(import_accounts_slim_text(args.payload).await?)
        }
        "export_accounts_full_encrypted_bytes" => {
            let args: ExportEncryptedArgs = parse_args(payload)?;
            let encoded =
                STANDARD.encode(export_accounts_full_encrypted_bytes(args.passphrase).await?);
            to_json(encoded)
        }
        "import_accounts_full_encrypted_bytes" => {
            let args: UploadEncryptedArgs = parse_args(payload)?;
            let bytes = STANDARD
                .decode(args.contents_base64)
                .map_err(|error| format!("Failed to decode uploaded backup: {error}"))?;
            to_json(import_accounts_full_encrypted_bytes(bytes, args.passphrase).await?)
        }
        "get_masked_account_ids" => to_json(get_masked_account_ids().await?),
        "set_masked_account_ids" => {
            let args: MaskedIdsArgs = parse_args(payload)?;
            to_json(set_masked_account_ids(args.ids).await?)
        }
        "check_codex_processes" => to_json(check_codex_processes().await?),
        "kill_codex_processes" => to_json(kill_codex_processes().await?),
        _ => Err(format!("Unsupported web command: {command}")),
    }
}

fn parse_request_json(request: &mut Request) -> anyhow::Result<Value> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .context("Failed to read request body")?;

    if body.trim().is_empty() {
        return Ok(json!({}));
    }

    serde_json::from_str(&body).context("Failed to parse request JSON")
}

fn parse_args<T>(value: Value) -> Result<T, String>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).map_err(|error| format!("Invalid command payload: {error}"))
}

fn to_json<T>(value: T) -> Result<Value, String>
where
    T: serde::Serialize,
{
    serde_json::to_value(value).map_err(|error| format!("Failed to serialize response: {error}"))
}

fn serve_static(
    request: Request,
    dist_dir: &Path,
    url: &str,
    security: &WebSecurity,
) -> anyhow::Result<()> {
    let requested = if url == "/" {
        PathBuf::from("index.html")
    } else {
        sanitize_path(url)?
    };
    let candidate = dist_dir.join(&requested);

    if candidate.is_file() {
        return serve_file(request, candidate, security, url);
    }

    if requested.extension().is_some() {
        respond_text(
            request,
            StatusCode(404),
            "Not Found",
            "text/plain; charset=utf-8",
            None,
        )?;
        return Ok(());
    }

    serve_file(request, dist_dir.join("index.html"), security, url)
}

fn sanitize_path(url: &str) -> anyhow::Result<PathBuf> {
    let path = url.split('?').next().unwrap_or("/");
    let raw = path.trim_start_matches('/');
    let candidate = Path::new(raw);

    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => anyhow::bail!("Invalid request path"),
        }
    }

    Ok(candidate.to_path_buf())
}

fn serve_file(
    request: Request,
    path: PathBuf,
    security: &WebSecurity,
    url: &str,
) -> anyhow::Result<()> {
    let data = fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mime = mime_type_for_path(&path);
    let mut response = Response::from_data(data)
        .with_header(header("Content-Type", mime)?)
        .with_header(header("Cache-Control", "no-cache")?);
    if url_has_valid_token(url, security) {
        if let Some(token) = security.token() {
            response = response.with_header(header(
                "Set-Cookie",
                &format!("{WEB_TOKEN_COOKIE}={token}; Path=/; SameSite=Strict; HttpOnly"),
            )?);
        }
    }
    request.respond(response)?;
    Ok(())
}

fn respond_json(request: Request, status: StatusCode, payload: &Value) -> anyhow::Result<()> {
    let response = Response::from_string(serde_json::to_string(payload)?)
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json; charset=utf-8")?);
    request.respond(response)?;
    Ok(())
}

fn respond_text(
    request: Request,
    status: StatusCode,
    body: &str,
    content_type: &str,
    extra_header: Option<Header>,
) -> anyhow::Result<()> {
    let mut response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header("Content-Type", content_type)?);
    if let Some(extra_header) = extra_header {
        response = response.with_header(extra_header);
    }
    request.respond(response)?;
    Ok(())
}

fn request_headers(request: &Request) -> Vec<(String, String)> {
    request
        .headers()
        .iter()
        .map(|header| {
            (
                header.field.to_string().to_ascii_lowercase(),
                header.value.as_str().to_string(),
            )
        })
        .collect()
}

fn request_is_authorized(
    url: &str,
    headers: &[(impl AsRef<str>, impl AsRef<str>)],
    security: &WebSecurity,
) -> bool {
    security.token().is_none()
        || url_has_valid_token(url, security)
        || request_has_valid_web_token(headers, security)
}

pub(crate) fn request_has_valid_web_token(
    headers: &[(impl AsRef<str>, impl AsRef<str>)],
    security: &WebSecurity,
) -> bool {
    let Some(token) = security.token() else {
        return true;
    };

    headers.iter().any(|(name, value)| {
        let name = name.as_ref();
        let value = value.as_ref();
        if name.eq_ignore_ascii_case(WEB_TOKEN_HEADER) {
            return value == token;
        }

        name.eq_ignore_ascii_case("cookie") && cookie_contains_token(value, token)
    })
}

fn cookie_contains_token(cookie_header: &str, token: &str) -> bool {
    cookie_header
        .split(';')
        .map(str::trim)
        .any(|part| part == format!("{WEB_TOKEN_COOKIE}={token}"))
}

fn url_has_valid_token(url: &str, security: &WebSecurity) -> bool {
    let Some(token) = security.token() else {
        return true;
    };

    let Some(query) = url.split_once('?').map(|(_, query)| query) else {
        return false;
    };

    query.split('&').any(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        name == "token" && value == token
    })
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn generate_web_token() -> String {
    let mut bytes = [0u8; WEB_TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn header(name: &str, value: &str) -> anyhow::Result<Header> {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).map_err(|_| {
        anyhow::anyhow!("Failed to create header {name}: invalid header value `{value}`")
    })
}

fn mime_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "ico" => "image/x-icon",
        "jpeg" | "jpg" => "image/jpeg",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "txt" => "text/plain; charset=utf-8",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{request_has_valid_web_token, web_security_for_host, WEB_TOKEN_HEADER};

    #[test]
    fn localhost_web_server_does_not_require_token_by_default() {
        let security = web_security_for_host("127.0.0.1", None).expect("security config");

        assert!(security.token().is_none());
    }

    #[test]
    fn non_loopback_web_server_requires_token() {
        let security = web_security_for_host("0.0.0.0", None).expect("security config");

        assert!(security.token().is_some());
    }

    #[test]
    fn web_token_validation_accepts_header_or_cookie_only_when_matching() {
        let security = web_security_for_host("0.0.0.0", Some("secret-token".to_string()))
            .expect("security config");

        assert!(request_has_valid_web_token(
            &[("x-codex-switcher-token", "secret-token")],
            &security
        ));
        assert!(request_has_valid_web_token(
            &[(
                "cookie",
                "theme=dark; codex_switcher_web_token=secret-token"
            )],
            &security
        ));
        assert!(!request_has_valid_web_token(
            &[(WEB_TOKEN_HEADER, "wrong-token")],
            &security
        ));
        let empty_headers: Vec<(&str, &str)> = Vec::new();
        assert!(!request_has_valid_web_token(&empty_headers, &security));
    }
}
