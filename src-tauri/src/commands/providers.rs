//! Discover model IDs exposed by an OpenAI-compatible provider.

use crate::types::CustomProvider;
use serde::Deserialize;
use std::time::Duration;

const MAX_MODEL_LIST_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct ModelList {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
}

#[tauri::command]
pub async fn list_provider_models(
    api_key: String,
    base_url: String,
) -> Result<Vec<String>, String> {
    if api_key.trim().is_empty() {
        return Err("An API key is required to list models".into());
    }
    CustomProvider {
        name: "Model discovery".into(),
        base_url: base_url.clone(),
        model: "discovery".into(),
    }
    .validate()
    .map_err(|error| error.to_string())?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Could not create provider connection".to_string())?;
    let mut response = client
        .get(format!("{}/models", base_url.trim_end_matches('/')))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|_| "Could not reach provider model list".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Provider model list returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MODEL_LIST_BYTES as u64)
    {
        return Err("Provider model list is too large (maximum 1 MiB)".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read provider model list".to_string())?
    {
        if chunk.len() > MAX_MODEL_LIST_BYTES - body.len() {
            return Err("Provider model list is too large (maximum 1 MiB)".into());
        }
        body.extend_from_slice(&chunk);
    }
    let list: ModelList = serde_json::from_slice(&body)
        .map_err(|_| "Provider returned an invalid model list".to_string())?;
    let mut ids: Vec<String> = list
        .data
        .into_iter()
        .map(|model| model.id)
        .filter(|id| !id.trim().is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    fn server(response: String) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (send, receive) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut connection = loop {
                if let Ok((connection, _)) = listener.accept() {
                    break connection;
                }
                if std::time::Instant::now() > deadline {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            connection.set_nonblocking(false).unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = connection.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let _ = send.send(String::from_utf8(request).unwrap());
            let _ = connection.write_all(response.as_bytes());
        });
        (url, receive, thread)
    }

    fn response(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn model_ids_are_fetched_sorted_and_deduplicated() {
        let (url, request, thread) = server(response(
            "200 OK",
            r#"{"data":[{"id":"z-model"},{"id":"a-model"},{"id":"z-model"}]}"#,
        ));
        let models = list_provider_models("fake-test-key".into(), format!("{url}/"))
            .await
            .unwrap();
        assert_eq!(models, vec!["a-model", "z-model"]);
        let request = request.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer fake-test-key\r\n"));
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn status_and_parse_errors_do_not_echo_response_secrets() {
        for (status, body, expected) in [
            ("401 Unauthorized", "fake-test-key private detail", "401"),
            ("200 OK", "fake-test-key invalid json", "invalid model list"),
            ("200 OK", r#"{"data":[{"id":42}]}"#, "invalid model list"),
        ] {
            let (url, _, thread) = server(response(status, body));
            let error = list_provider_models("fake-test-key".into(), url)
                .await
                .unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("fake-test-key"));
            thread.join().unwrap();
        }
    }

    #[tokio::test]
    async fn oversized_model_list_is_rejected() {
        let body = "x".repeat(1024 * 1024 + 1);
        for reply in [
            response("200 OK", &body),
            format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}"),
        ] {
            let (url, _, thread) = server(reply);
            let error = list_provider_models("fake-test-key".into(), url)
                .await
                .unwrap_err();
            assert!(error.contains("too large"), "{error}");
            thread.join().unwrap();
        }
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let (url, _, thread) = server(format!("HTTP/1.1 302 Found\r\nLocation: http://{}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", target.local_addr().unwrap()));
        let error = list_provider_models("fake-test-key".into(), url)
            .await
            .unwrap_err();
        assert!(error.contains("302"), "{error}");
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn invalid_settings_are_rejected_before_request() {
        assert!(
            list_provider_models(" ".into(), "http://localhost/v1".into())
                .await
                .unwrap_err()
                .contains("API key")
        );
        assert!(
            list_provider_models("fake-test-key".into(), "http://example.com/v1".into())
                .await
                .is_err()
        );
        assert!(list_provider_models(
            "fake-test-key".into(),
            "https://user:secret@example.com/v1".into()
        )
        .await
        .is_err());
    }
}
