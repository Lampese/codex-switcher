use std::fs;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;

use anyhow::Context;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use tokio::runtime::Runtime;

use crate::commands::{
    add_account_from_auth_json_text, add_account_from_file, cancel_login, check_codex_processes,
    complete_login, delete_account, export_accounts_full_encrypted_bytes,
    export_accounts_slim_text, fetch_usage, get_account_usage_stats, get_active_account_info,
    get_masked_account_ids, import_accounts_full_encrypted_bytes, import_accounts_slim_text,
    kill_codex_processes, list_accounts, refresh_account_metadata, refresh_all_accounts_usage,
    rename_account, set_masked_account_ids, start_login, switch_account, warmup_account,
    warmup_all_accounts,
};

const WEB_WORKER_THREADS: usize = 4;
const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

#[cfg(test)]
static TEST_SLOW_COMMAND_STARTED: OnceLock<Mutex<Option<mpsc::Sender<()>>>> = OnceLock::new();
#[cfg(test)]
static TEST_SLOW_COMMAND_RELEASE: OnceLock<Mutex<Option<mpsc::Receiver<()>>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct WebAuth {
    secret: Option<String>,
}

impl WebAuth {
    fn from_host(host: &str) -> anyhow::Result<Self> {
        let secret = std::env::var("CODEX_SWITCHER_WEB_SECRET")
            .ok()
            .filter(|value| !value.is_empty());
        Self::from_host_and_secret(host, secret)
    }

    fn from_host_and_secret(host: &str, secret: Option<String>) -> anyhow::Result<Self> {
        if !is_loopback_host(host) && secret.is_none() {
            anyhow::bail!(
                "Non-loopback web binding requires CODEX_SWITCHER_WEB_SECRET; use an encrypted tunnel or TLS for remote access"
            );
        }
        Ok(Self { secret })
    }

    fn requires_auth(&self) -> bool {
        self.secret.is_some()
    }
}

fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().trim_start_matches('[').trim_end_matches(']');
    normalized.eq_ignore_ascii_case("localhost")
        || normalized
            .parse::<IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false)
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

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseCodexArgs {
    reopen_desktop: Option<bool>,
    force_close: Option<bool>,
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
}

#[derive(Debug, Deserialize)]
struct FileImportArgs {
    path: String,
    name: String,
}

pub fn run_lan_server(host: &str, port: u16) -> anyhow::Result<()> {
    let address = format!("{host}:{port}");
    let auth = Arc::new(WebAuth::from_host(host)?);
    let server = Arc::new(
        Server::http(&address)
            .map_err(|err| anyhow::anyhow!("Failed to bind HTTP server on {address}: {err}"))?,
    );
    let runtime = Arc::new(Runtime::new().context("Failed to start async runtime")?);
    let dist_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("dist");

    println!("Codex Switcher web server listening on http://{address}");
    println!("Serving static files from {}", dist_dir.display());

    let workers = spawn_web_workers(server.clone(), runtime, dist_dir, auth);

    for worker in workers {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("Web worker thread panicked"))?;
    }

    Ok(())
}

fn spawn_web_workers(
    server: Arc<Server>,
    runtime: Arc<Runtime>,
    dist_dir: PathBuf,
    auth: Arc<WebAuth>,
) -> Vec<thread::JoinHandle<()>> {
    let mut workers = Vec::with_capacity(WEB_WORKER_THREADS);
    for _ in 0..WEB_WORKER_THREADS {
        let server = Arc::clone(&server);
        let runtime = Arc::clone(&runtime);
        let auth = Arc::clone(&auth);
        let dist_dir = dist_dir.clone();
        workers.push(thread::spawn(move || loop {
            match server.recv() {
                Ok(request) => {
                    if let Err(error) = handle_request(request, &runtime, &dist_dir, &auth) {
                        eprintln!("[web] request failed: {error:#}");
                    }
                }
                Err(error) => {
                    eprintln!("[web] request receive failed: {error}");
                    break;
                }
            }
        }));
    }
    workers
}

fn handle_request(
    mut request: Request,
    runtime: &Runtime,
    dist_dir: &Path,
    auth: &WebAuth,
) -> anyhow::Result<()> {
    let method = request.method().clone();
    let url = request.url().to_string();

    if method == Method::Get && url == "/api/health" {
        respond_json(request, StatusCode(200), &json!({ "ok": true }))?;
        return Ok(());
    }

    if method == Method::Post && url.starts_with("/api/invoke/") {
        if !is_authorized(&request, auth) {
            respond_unauthorized(request)?;
            return Ok(());
        }
        let command = url.trim_start_matches("/api/invoke/");
        let payload = match parse_request_json(&mut request) {
            Ok(payload) => payload,
            Err(error) => {
                let status = if matches!(error, RequestBodyError::TooLarge) {
                    StatusCode(413)
                } else {
                    StatusCode(400)
                };
                respond_json(request, status, &json!({ "error": error.to_string() }))?;
                return Ok(());
            }
        };
        let result = runtime.block_on(invoke_web_command(command, payload));
        match result {
            Ok(value) => respond_json(request, StatusCode(200), &value)?,
            Err(error) => respond_json(request, StatusCode(400), &json!({ "error": error }))?,
        }
        return Ok(());
    }

    if method == Method::Get {
        serve_static(request, dist_dir, &url)?;
        return Ok(());
    }

    respond_text(
        request,
        StatusCode(405),
        "Method Not Allowed",
        "text/plain; charset=utf-8",
    )?;
    Ok(())
}

fn is_authorized(request: &Request, auth: &WebAuth) -> bool {
    let Some(expected) = auth.secret.as_deref() else {
        return !auth.requires_auth();
    };
    let provided = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .and_then(|header| header.value.as_str().strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();
    constant_time_eq(expected.as_bytes(), provided.as_bytes())
}

fn constant_time_eq(expected: &[u8], provided: &[u8]) -> bool {
    let mut difference = expected.len() ^ provided.len();
    for index in 0..expected.len().max(provided.len()) {
        let left = expected.get(index).copied().unwrap_or_default();
        let right = provided.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

fn respond_unauthorized(request: Request) -> anyhow::Result<()> {
    let response = Response::from_string(r#"{"error":"Unauthorized"}"#)
        .with_status_code(StatusCode(401))
        .with_header(header("Content-Type", "application/json; charset=utf-8")?)
        .with_header(header("WWW-Authenticate", "Bearer")?);
    request.respond(response)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum RequestBodyError {
    #[error("Request body exceeds the 2097152-byte limit")]
    TooLarge,
    #[error("Failed to read request body: {0}")]
    Read(#[from] std::io::Error),
    #[error("Request body is not valid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("Failed to parse request JSON: {0}")]
    Json(#[from] serde_json::Error),
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
            to_json(fetch_usage(&args.account_id).await?)
        }
        "get_account_usage_stats" => {
            let args: AccountIdArgs = parse_args(payload)?;
            to_json(get_account_usage_stats(args.account_id).await?)
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
        #[cfg(test)]
        "__test_slow" => {
            if let Some(lock) = TEST_SLOW_COMMAND_STARTED.get() {
                if let Some(sender) = lock.lock().unwrap().take() {
                    let _ = sender.send(());
                }
            }
            if let Some(lock) = TEST_SLOW_COMMAND_RELEASE.get() {
                if let Some(receiver) = lock.lock().unwrap().take() {
                    let _ = receiver.recv();
                }
            }
            Ok(json!({ "ok": true }))
        }
        "cancel_login" => to_json(cancel_login().await?),
        "export_accounts_slim_text" => to_json(export_accounts_slim_text().await?),
        "import_accounts_slim_text" => {
            let args: ImportSlimArgs = parse_args(payload)?;
            to_json(import_accounts_slim_text(args.payload).await?)
        }
        "export_accounts_full_encrypted_bytes" => {
            let encoded = STANDARD.encode(export_accounts_full_encrypted_bytes().await?);
            to_json(encoded)
        }
        "import_accounts_full_encrypted_bytes" => {
            let args: UploadEncryptedArgs = parse_args(payload)?;
            let bytes = STANDARD
                .decode(args.contents_base64)
                .map_err(|error| format!("Failed to decode uploaded backup: {error}"))?;
            to_json(import_accounts_full_encrypted_bytes(bytes).await?)
        }
        "get_masked_account_ids" => to_json(get_masked_account_ids().await?),
        "set_masked_account_ids" => {
            let args: MaskedIdsArgs = parse_args(payload)?;
            to_json(set_masked_account_ids(args.ids).await?)
        }
        "check_codex_processes" => to_json(check_codex_processes().await?),
        "kill_codex_processes" => {
            let args: CloseCodexArgs = parse_args(payload)?;
            to_json(kill_codex_processes(args.reopen_desktop, args.force_close).await?)
        }
        _ => Err(format!("Unsupported web command: {command}")),
    }
}

fn parse_request_json(request: &mut Request) -> Result<Value, RequestBodyError> {
    if request
        .body_length()
        .is_some_and(|length| length > MAX_REQUEST_BODY_BYTES)
    {
        return Err(RequestBodyError::TooLarge);
    }

    let mut bytes = Vec::new();
    request
        .as_reader()
        .take((MAX_REQUEST_BODY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST_BODY_BYTES {
        return Err(RequestBodyError::TooLarge);
    }
    parse_request_bytes(&bytes)
}

fn parse_request_bytes(bytes: &[u8]) -> Result<Value, RequestBodyError> {
    if bytes.len() > MAX_REQUEST_BODY_BYTES {
        return Err(RequestBodyError::TooLarge);
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    let body = String::from_utf8(bytes.to_vec())?;
    Ok(serde_json::from_str(&body)?)
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

fn serve_static(request: Request, dist_dir: &Path, url: &str) -> anyhow::Result<()> {
    let requested = if url == "/" {
        PathBuf::from("index.html")
    } else {
        sanitize_path(url)?
    };
    let candidate = dist_dir.join(&requested);

    if candidate.is_file() {
        return serve_file(request, candidate);
    }

    if requested.extension().is_some() {
        respond_text(
            request,
            StatusCode(404),
            "Not Found",
            "text/plain; charset=utf-8",
        )?;
        return Ok(());
    }

    serve_file(request, dist_dir.join("index.html"))
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

fn serve_file(request: Request, path: PathBuf) -> anyhow::Result<()> {
    let data = fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mime = mime_type_for_path(&path);
    let response = Response::from_data(data)
        .with_header(header("Content-Type", mime)?)
        .with_header(header("Cache-Control", "no-cache")?);
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
) -> anyhow::Result<()> {
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header("Content-Type", content_type)?);
    request.respond(response)?;
    Ok(())
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
    use std::io::{Read, Write};
    use std::net::{Shutdown, SocketAddr, TcpStream};
    use std::time::Duration;

    use super::*;

    #[test]
    fn loopback_hosts_do_not_require_a_secret() {
        assert!(WebAuth::from_host_and_secret("127.0.0.1", None).is_ok());
        assert!(WebAuth::from_host_and_secret("::1", None).is_ok());
        assert!(WebAuth::from_host_and_secret("localhost", None).is_ok());
    }

    #[test]
    fn non_loopback_hosts_fail_closed_without_a_secret() {
        let error = WebAuth::from_host_and_secret("0.0.0.0", None).unwrap_err();
        assert!(error.to_string().contains("CODEX_SWITCHER_WEB_SECRET"));
        assert!(WebAuth::from_host_and_secret("0.0.0.0", Some("secret".into())).is_ok());
    }

    #[test]
    fn bearer_auth_uses_constant_time_value_comparison() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"wrong"));
        assert!(!constant_time_eq(b"secret", b"secret-longer"));
    }

    #[test]
    fn request_body_parser_rejects_oversized_payloads_before_json_parsing() {
        let oversized = vec![b' '; MAX_REQUEST_BODY_BYTES + 1];
        assert!(matches!(
            parse_request_bytes(&oversized),
            Err(RequestBodyError::TooLarge)
        ));
        assert_eq!(parse_request_bytes(b"{}\n").unwrap(), json!({}));
    }

    #[test]
    fn slow_request_does_not_block_health_request() {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let address = server.server_addr().to_ip().unwrap();
        let runtime = Arc::new(Runtime::new().unwrap());
        let dist_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("dist");
        let auth = Arc::new(WebAuth::from_host_and_secret("127.0.0.1", None).unwrap());
        let workers = spawn_web_workers(server.clone(), runtime, dist_dir, auth);

        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        *TEST_SLOW_COMMAND_STARTED
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(started_sender);
        *TEST_SLOW_COMMAND_RELEASE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(release_receiver);

        let slow_request = thread::spawn(move || {
            send_http_request(address, "POST", "/api/invoke/__test_slow", "{}")
        });
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("slow command should occupy a worker");

        let health_response = send_http_request(address, "GET", "/api/health", "");
        assert!(
            health_response.starts_with("HTTP/1.1 200 OK"),
            "health response: {health_response}"
        );

        release_sender.send(()).unwrap();
        let slow_response = slow_request.join().unwrap();
        assert!(slow_response.starts_with("HTTP/1.1 200 OK"));

        for _ in 0..WEB_WORKER_THREADS {
            server.unblock();
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }

    fn send_http_request(address: SocketAddr, method: &str, path: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream.shutdown(Shutdown::Write).unwrap();

        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8(response).unwrap()
    }
}
