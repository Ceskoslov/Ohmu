//! Explicit URL imports with per-hop destination validation and DNS pinning.
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use ohmu_core::MAX_SOURCE_BYTES;
use reqwest::{Url, blocking::Client, header, redirect::Policy};

use crate::{EngineError, IngestReport, Wiki, parsers::ParserKind, sources::source_error};

impl Wiki {
    /// Fetch one public HTTP(S) resource, following at most five validated redirects.
    /// No proxies, credentials, automatic retries or subresource fetching.
    pub fn ingest_url(&self, address: &str) -> Result<IngestReport, EngineError> {
        self.require_recovered()?;
        self.import_url(address, false)
    }

    fn import_url(
        &self,
        address: &str,
        allow_test_loopback: bool,
    ) -> Result<IngestReport, EngineError> {
        let mut url = checked_url(address)?;
        let origin = url.to_string();
        let deadline = Instant::now() + Duration::from_secs(30);
        for hop in 0..=5 {
            let host = url
                .host_str()
                .ok_or_else(|| source_error("URL has no host"))?;
            let port = url
                .port_or_known_default()
                .ok_or_else(|| source_error("URL has no port"))?;
            let host = host.trim_start_matches('[').trim_end_matches(']');
            let addresses: Vec<SocketAddr> = (host, port)
                .to_socket_addrs()
                .map_err(source_error)?
                .collect();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| !allowed_ip(address.ip(), allow_test_loopback))
            {
                return Err(source_error(
                    "URL destination is private, reserved, local or unresolved",
                ));
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| source_error("URL import deadline exceeded"))?;
            // Pin the validated DNS result, including on every redirect. Keep the
            // original hostname for Host/TLS verification. Disable environment proxies.
            let client = Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .retry(reqwest::retry::never())
                .timeout(remaining)
                .connect_timeout(remaining.min(Duration::from_secs(10)))
                .resolve_to_addrs(host, &addresses)
                .pool_max_idle_per_host(0)
                .build()
                .map_err(http_error)?;
            let mut response = client
                .get(url.clone())
                .header(
                    header::ACCEPT,
                    "text/html, application/pdf, text/markdown, text/plain",
                )
                .send()
                .map_err(http_error)?;
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                if hop == 5 {
                    return Err(source_error("URL redirect limit exceeded"));
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .ok_or_else(|| source_error("redirect has no Location header"))?
                    .to_str()
                    .map_err(source_error)?;
                url = checked_url(url.join(location).map_err(source_error)?.as_str())?;
                continue;
            }
            if !response.status().is_success() {
                return Err(source_error(format!(
                    "URL returned HTTP {}",
                    response.status().as_u16()
                )));
            }
            if response
                .headers()
                .get(header::CONTENT_ENCODING)
                .is_some_and(|value| value != "identity")
            {
                return Err(source_error(
                    "encoded HTTP bodies are unsupported; provide an identity response",
                ));
            }
            let media = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if media.contains("charset=")
                && !media.contains("charset=utf-8")
                && !media.contains("charset=\"utf-8\"")
                && !media.contains("charset=us-ascii")
            {
                return Err(source_error("HTTP text must use UTF-8 or US-ASCII"));
            }
            let kind = match media.split(';').next().unwrap_or("").trim() {
                "text/html" | "application/xhtml+xml" => ParserKind::Html,
                "application/pdf" => ParserKind::Pdf,
                "text/markdown" | "text/plain" => ParserKind::Markdown,
                _ => return Err(source_error("unsupported or missing HTTP Content-Type")),
            };
            if response
                .content_length()
                .is_some_and(|length| length > MAX_SOURCE_BYTES as u64)
            {
                return Err(EngineError::TooLarge {
                    maximum: MAX_SOURCE_BYTES,
                });
            }
            let mut bytes = Vec::new();
            Read::by_ref(&mut response)
                .take(MAX_SOURCE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if Instant::now() >= deadline {
                return Err(source_error("URL import deadline exceeded"));
            }
            return self.ingest_bytes(&origin, &bytes, kind, Some(url.to_string()));
        }
        unreachable!("redirect loop always returns")
    }
}

fn checked_url(address: &str) -> Result<Url, EngineError> {
    if address.len() > 8192 {
        return Err(source_error("URL exceeds 8192 bytes"));
    }
    let mut url = Url::parse(address).map_err(source_error)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(source_error(
            "URL must be HTTP(S) without embedded credentials",
        ));
    }
    url.set_fragment(None);
    if url.as_str().len() > 8192 {
        return Err(source_error("normalized URL exceeds 8192 bytes"));
    }
    Ok(url)
}

fn allowed_ip(ip: IpAddr, allow_test_loopback: bool) -> bool {
    // Only unit-test call sites can request loopback. The public API always
    // passes false; no CLI/config switch can weaken destination validation.
    if cfg!(test) && allow_test_loopback && ip.is_loopback() {
        return true;
    }
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || (b == 0 && (c == 0 || c == 2))))
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && s[0] != 0x3fff
                && !(s[0] == 0x2001 && (s[1] <= 0x01ff || s[1] == 0x0db8))
        }
    }
}

fn http_error(error: reqwest::Error) -> EngineError {
    source_error(format!("HTTP request failed: {}", error.without_url()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    fn serve(responses: Vec<String>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            for response in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("fixture server: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 8192];
                let _ = stream.read(&mut request).unwrap();
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (address, handle)
    }

    fn response(code: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {code}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn public_policy_rejects_local_reserved_and_credentials() {
        for ip in [
            "127.0.0.1",
            "10.1.1.1",
            "169.254.169.254",
            "100.64.1.1",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2001:db8::1",
            "2002:a00:1::",
        ] {
            assert!(!allowed_ip(ip.parse().unwrap(), false), "{ip}");
        }
        assert!(allowed_ip("8.8.8.8".parse().unwrap(), false));
        assert!(allowed_ip("2606:4700:4700::1111".parse().unwrap(), false));
        for url in [
            "file:///etc/passwd",
            "ftp://host/a",
            "http://user:secret@example.org/",
        ] {
            assert!(checked_url(url).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
        assert!(wiki.ingest_url("http://127.0.0.1:1/a").is_err());
    }

    #[test]
    fn redirect_versions_and_rebuild_preserve_requested_and_final_urls() {
        let temp = tempfile::tempdir().unwrap();
        let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
        let (base, server) = serve(vec![
            response("302 Found", "Location: /article\r\n", ""),
            response(
                "200 OK",
                "Content-Type: text/markdown; charset=utf-8\r\n",
                "# First\nEvidence\n",
            ),
            response("302 Found", "Location: /article\r\n", ""),
            response(
                "200 OK",
                "Content-Type: text/markdown\r\n",
                "# Second\nNew evidence\n",
            ),
        ]);
        let first = wiki
            .import_url(&format!("{base}/start#ignored"), true)
            .unwrap();
        let second = wiki.import_url(&format!("{base}/start"), true).unwrap();
        server.join().unwrap();
        assert_eq!(first.record.origin, format!("{base}/start"));
        assert_eq!(
            first.record.resolved_url.as_deref(),
            Some(format!("{base}/article").as_str())
        );
        assert_eq!(first.record.source_id, second.record.source_id);
        assert_ne!(first.record.revision, second.record.revision);
        assert_eq!(wiki.rebuild_source_catalog().unwrap(), 2);
        assert!(
            wiki.source_document(&first.record)
                .unwrap()
                .text
                .contains("First")
        );
    }

    #[test]
    fn failed_unsupported_oversized_and_private_redirect_responses_never_commit() {
        for (headers, code) in [
            ("Content-Type: application/octet-stream\r\n", "200 OK"),
            ("Content-Type: text/plain\r\n", "503 Unavailable"),
            ("Location: http://10.0.0.1/blocked\r\n", "302 Found"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
            let (base, server) = serve(vec![response(code, headers, "no")]);
            assert!(wiki.import_url(&base, true).is_err());
            server.join().unwrap();
            assert!(wiki.sources().unwrap().is_empty());
        }
        let temp = tempfile::tempdir().unwrap();
        let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
        let (base, server) = serve(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_SOURCE_BYTES + 1
        )]);
        assert!(wiki.import_url(&base, true).is_err());
        server.join().unwrap();
    }

    #[test]
    fn redirects_are_bounded_and_connection_failures_do_not_create_sources() {
        let temp = tempfile::tempdir().unwrap();
        let wiki = Wiki::init(temp.path().join("wiki"), "test").unwrap();
        let (base, server) = serve(vec![response("302 Found", "Location: /loop\r\n", ""); 6]);
        assert!(
            wiki.import_url(&base, true)
                .unwrap_err()
                .to_string()
                .contains("redirect limit")
        );
        server.join().unwrap();
        assert!(wiki.import_url("http://127.0.0.1:0/", true).is_err());
        assert!(wiki.sources().unwrap().is_empty());
    }
}
