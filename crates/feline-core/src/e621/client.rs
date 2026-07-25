use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use wreq::header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderValue, USER_AGENT};
use wreq_util::Profile;

use super::rate_limit::{ApiLimiter, new_api_limiter};
use super::types::{Post, PostsResponse};
use crate::config::{MediaSkip, RatingFilter, Site};
use crate::credentials::Credentials;
use crate::media_cache::{self, MAX_CACHE_BYTES};
use crate::util::safe_truncate;

const MAX_LIMIT: u32 = 320;
const EMULATION_PROFILE: Profile = Profile::Chrome136;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_API_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_MEDIA_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;

pub const SESSION_EXPIRED: &str = "Your session expired. Sign in again.";

static DOWNLOAD_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct RawResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

struct DownloadTempFile {
    path: PathBuf,
    armed: bool,
}

impl DownloadTempFile {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DownloadTempFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone)]
pub struct Client {
    api_http: wreq::Client,
    download_http: wreq::Client,
    limiter: Arc<ApiLimiter>,
    site: Site,
    creds: Option<Credentials>,
    cache_dir: Option<PathBuf>,
}

impl Client {
    pub async fn new(site: Site, creds: Option<Credentials>) -> Result<Self> {
        Self::with_limiter(site, creds, new_api_limiter(), None).await
    }

    pub async fn with_limiter(
        site: Site,
        creds: Option<Credentials>,
        limiter: Arc<ApiLimiter>,
        proxy_url: Option<String>,
    ) -> Result<Self> {
        let ua = build_user_agent(creds.as_ref());

        let api_http = build_wreq(&ua, proxy_url.as_deref()).context("build API wreq client")?;
        let download_http =
            build_download_wreq(&ua, proxy_url.as_deref()).context("build download wreq client")?;

        Ok(Self {
            api_http,
            download_http,
            limiter,
            site,
            creds,
            cache_dir: None,
        })
    }

    pub fn set_cache_dir(&mut self, dir: Option<PathBuf>) {
        self.cache_dir = dir;
    }

    pub fn http(&self) -> &wreq::Client {
        &self.download_http
    }

    fn url(&self, path: &str) -> String {
        format!("https://{}/{path}", self.site.host())
    }

    fn apply_auth(&self, req: wreq::RequestBuilder) -> wreq::RequestBuilder {
        match &self.creds {
            Some(creds) if !creds.is_empty() => {
                req.basic_auth(&creds.username, Some(&creds.api_key))
            }
            _ => req,
        }
    }

    pub async fn verify_login(&self) -> Result<()> {
        let creds = self
            .creds
            .as_ref()
            .ok_or_else(|| anyhow!("no credentials to verify"))?;
        if creds.is_empty() {
            return Err(anyhow!("username and API key are required"));
        }

        self.limiter.until_ready().await;

        let resp = self
            .api_http
            .get(self.url("favorites.json"))
            .query(&[("limit", "1")])
            .basic_auth(&creds.username, Some(&creds.api_key))
            .send()
            .await
            .context("send login check")?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(anyhow!("invalid username or API key"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if status.as_u16() == 403 && is_cloudflare_challenge(&body) {
                return Err(anyhow!(
                    "Cloudflare blocked the request (the network or VPN you're using is flagged). Try a different exit node."
                ));
            }
            if status.as_u16() == 403 {
                return Err(anyhow!("invalid username or API key"));
            }
            return Err(anyhow!(
                "login check failed: HTTP {} {}",
                status,
                safe_truncate(&body, 200)
            ));
        }
        Ok(())
    }

    pub async fn search_page(
        &self,
        tags: &str,
        blacklist: &[String],
        rating: RatingFilter,
        media_skip: MediaSkip,
        before_id: Option<u64>,
    ) -> Result<Vec<Post>> {
        let full_query = build_query_string(tags, blacklist, rating, media_skip);

        self.limiter.until_ready().await;

        let params = search_params(full_query, before_id);
        let req = self.api_http.get(self.url("posts.json")).query(&params);

        let resp = self
            .apply_auth(req)
            .send()
            .await
            .context("send search request")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if status.as_u16() == 403 && is_cloudflare_challenge(&body) {
                return Err(anyhow!(
                    "Cloudflare blocked the request. Try a different VPN exit node."
                ));
            }
            return Err(anyhow!(
                "search failed: HTTP {} {}",
                status,
                safe_truncate(&body, 300)
            ));
        }

        let parsed: PostsResponse = resp.json().await.context("decode posts.json response")?;
        Ok(parsed.posts)
    }

    pub async fn raw_request(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
    ) -> anyhow::Result<RawResponse> {
        if !path.starts_with('/') {
            anyhow::bail!("path must start with '/'");
        }
        let url = format!("https://{}{}", self.site.host(), path);
        self.limiter.until_ready().await;

        let mut req = match method {
            "GET" => self.api_http.get(&url),
            "POST" => self.api_http.post(&url),
            "DELETE" => self.api_http.delete(&url),
            "PUT" => self.api_http.put(&url),
            "PATCH" => self.api_http.patch(&url),
            other => anyhow::bail!("unsupported HTTP method {other}"),
        };

        if !query.is_empty() {
            req = req.query(query);
        }

        req = self.apply_auth(req);

        if let Some(b) = body {
            req = req.body(b);
            if let Some(ct) = content_type {
                req = req.header(wreq::header::CONTENT_TYPE, ct);
            }
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let bytes = read_response_capped(resp, MAX_API_RESPONSE_BYTES).await?;

        if status == 403
            && let Ok(text) = std::str::from_utf8(&bytes)
            && is_cloudflare_challenge(text)
        {
            return Err(anyhow!(
                "Cloudflare blocked the request. Try a different VPN exit node."
            ));
        }

        Ok(RawResponse {
            status,
            body: bytes,
        })
    }

    pub async fn fetch_media(&self, url: &str) -> Result<RawResponse> {
        let parsed = url::Url::parse(url).context("invalid media URL")?;
        if parsed.scheme() != "https" {
            anyhow::bail!("media URL must use https");
        }
        let host = parsed.host_str().unwrap_or_default().to_string();
        if !is_allowed_media_host(&host) {
            anyhow::bail!("media host not allowed: {host}");
        }

        let cacheable = media_cache::is_cacheable_url(url);
        if cacheable
            && let Some(dir) = &self.cache_dir
            && let Some(bytes) = media_cache::read(dir, url)
        {
            return Ok(RawResponse {
                status: 200,
                body: bytes,
            });
        }

        let mut req = self.download_http.get(parsed.as_str());
        if host_accepts_credentials(&host, self.site.credential_domains()) {
            self.limiter.until_ready().await;
            req = self.apply_auth(req);
        }

        let resp = req.send().await.context("send media request")?;
        let status = resp.status().as_u16();
        if let Some(length) = parse_content_length(resp.headers())
            && length > MAX_MEDIA_RESPONSE_BYTES
        {
            anyhow::bail!("media response is too large ({length} bytes)");
        }

        let mut stream = resp.bytes_stream();
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("read media chunk")?;
            if bytes.len() as u64 + chunk.len() as u64 > MAX_MEDIA_RESPONSE_BYTES {
                anyhow::bail!("media response exceeds cap of {MAX_MEDIA_RESPONSE_BYTES} bytes");
            }
            bytes.extend_from_slice(&chunk);
        }

        if cacheable
            && status == 200
            && let Some(dir) = &self.cache_dir
        {
            let _ = media_cache::write(dir, url, &bytes, MAX_CACHE_BYTES);
        }

        Ok(RawResponse {
            status,
            body: bytes,
        })
    }

    pub async fn download_to_file(&self, url: &str, dest_path: &str) -> Result<u16> {
        use md5::{Digest, Md5};
        use tokio::io::AsyncWriteExt;

        let parsed = url::Url::parse(url).context("invalid media URL")?;
        if parsed.scheme() != "https" {
            anyhow::bail!("media URL must use https");
        }
        let host = parsed.host_str().unwrap_or_default().to_string();
        if !is_allowed_media_host(&host) {
            anyhow::bail!("media host not allowed: {host}");
        }

        let mut req = self.download_http.get(parsed.as_str());
        if host_accepts_credentials(&host, self.site.credential_domains()) {
            self.limiter.until_ready().await;
            req = self.apply_auth(req);
        }

        let resp = req.send().await.context("send media request")?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Ok(status);
        }

        if let Some(length) = parse_content_length(resp.headers())
            && length > MAX_DOWNLOAD_BYTES
        {
            anyhow::bail!("file is too large to download ({length} bytes)");
        }

        let expected_md5 = expected_md5_from_url(&parsed);

        let dest_path = Path::new(dest_path);
        let seq = DOWNLOAD_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let name = dest_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("download");
        let tmp_path =
            dest_path.with_file_name(format!(".{name}.{}.{seq}.part", std::process::id()));
        let mut tmp_file = DownloadTempFile::new(tmp_path.clone());

        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .context("create temporary destination file")?;
        let mut hasher = expected_md5.as_ref().map(|_| Md5::new());
        let mut total: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("read media chunk")?;
            total = total.saturating_add(chunk.len() as u64);
            if total > MAX_DOWNLOAD_BYTES {
                drop(file);
                anyhow::bail!("file exceeds download cap of {MAX_DOWNLOAD_BYTES} bytes; aborted");
            }
            if let Some(h) = hasher.as_mut() {
                h.update(&chunk);
            }
            file.write_all(&chunk).await.context("write media chunk")?;
        }
        file.flush().await.context("flush media file")?;
        file.sync_all().await.context("sync media file")?;
        drop(file);

        if let (Some(expected), Some(h)) = (expected_md5, hasher) {
            let actual = hex::encode(h.finalize());
            if actual != expected {
                anyhow::bail!("md5 mismatch: expected {expected}, got {actual}");
            }
        }
        tokio::fs::rename(&tmp_path, dest_path)
            .await
            .context("commit destination file")?;
        tmp_file.disarm();
        Ok(status)
    }
}

fn expected_md5_from_url(url: &url::Url) -> Option<String> {
    let last = url.path_segments()?.next_back()?;
    let stem = last.split('.').next()?;
    if stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(stem.to_ascii_lowercase())
    } else {
        None
    }
}

fn build_wreq(user_agent: &str, proxy_url: Option<&str>) -> Result<wreq::Client> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_str(user_agent)?);
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));

    let mut builder = wreq::Client::builder()
        .emulation(EMULATION_PROFILE)
        .default_headers(headers)
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(15));

    if let Some(url) = proxy_url {
        let proxy = wreq::Proxy::all(url).context("configure SOCKS5 proxy")?;
        builder = builder.proxy(proxy);
    }

    builder.build().context("build wreq client")
}

fn build_download_wreq(user_agent: &str, proxy_url: Option<&str>) -> Result<wreq::Client> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_str(user_agent)?);

    let mut builder = wreq::Client::builder()
        .emulation(EMULATION_PROFILE)
        .default_headers(headers)
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15));
    if let Some(url) = proxy_url {
        let proxy = wreq::Proxy::all(url).context("configure SOCKS5 proxy")?;
        builder = builder.proxy(proxy);
    }
    builder.build().context("build download wreq client")
}

fn host_accepts_credentials(host: &str, domains: &[&str]) -> bool {
    let host = host.trim_end_matches('.');
    domains
        .iter()
        .any(|domain| host.eq_ignore_ascii_case(domain))
}

fn is_allowed_media_host(host: &str) -> bool {
    Site::from_media_host(host).is_some()
}

fn search_params(full_query: String, before_id: Option<u64>) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> =
        vec![("tags", full_query), ("limit", MAX_LIMIT.to_string())];
    if let Some(id) = before_id {
        params.push(("page", format!("b{id}")));
    }
    params
}

fn parse_content_length(headers: &wreq::header::HeaderMap) -> Option<u64> {
    headers
        .get(wreq::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

async fn read_response_capped(resp: wreq::Response, max: u64) -> Result<Vec<u8>> {
    if let Some(length) = parse_content_length(resp.headers())
        && length > max
    {
        anyhow::bail!("response is too large ({length} bytes)");
    }

    let mut stream = resp.bytes_stream();
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read response chunk")?;
        if bytes.len() as u64 + chunk.len() as u64 > max {
            anyhow::bail!("response exceeds cap of {max} bytes");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn is_cloudflare_challenge(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("cdn-cgi/") || lower.contains("cloudflare") || lower.contains("just a moment")
}

pub fn build_user_agent(creds: Option<&Credentials>) -> String {
    match creds {
        Some(c) if !c.username.is_empty() => {
            format!("Feline/{APP_VERSION} (by {} on e621)", c.username)
        }
        _ => format!("Feline/{APP_VERSION} (portable)"),
    }
}

fn build_query_string(
    tags: &str,
    blacklist: &[String],
    rating: RatingFilter,
    media_skip: MediaSkip,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    let trimmed = tags.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }
    for b in blacklist {
        let b = b.trim();
        if b.is_empty() {
            continue;
        }
        if b.starts_with('-') {
            parts.push(b.to_string());
        } else {
            parts.push(format!("-{b}"));
        }
    }
    for token in media_skip.as_query_tokens() {
        parts.push(token.to_string());
    }
    if let Some(frag) = rating.as_query_fragment() {
        parts.push(frag);
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_accepts_credentials_for_e621_apex() {
        let domains = Site::E621.credential_domains();
        assert!(host_accepts_credentials("e621.net", domains));
        assert!(host_accepts_credentials("e926.net", domains));
    }

    #[test]
    fn host_accepts_credentials_for_subdomains() {
        let domains = Site::E621.credential_domains();
        assert!(!host_accepts_credentials("static1.e621.net", domains));
        assert!(!host_accepts_credentials("static1.e926.net", domains));
    }

    #[test]
    fn allowed_media_hosts_cover_known_sites() {
        assert!(is_allowed_media_host("static1.e621.net"));
        assert!(is_allowed_media_host("static2.e926.net"));
        assert!(!is_allowed_media_host("example.com"));
        assert!(!is_allowed_media_host("cdn.static1.e621.net"));
        assert!(!is_allowed_media_host("e621.net.evil.com"));
    }

    #[test]
    fn expected_md5_is_extracted_from_file_urls() {
        let md5 = "0123456789abcdef0123456789abcdef";
        let url =
            url::Url::parse(&format!("https://static1.e621.net/data/01/23/{md5}.png")).unwrap();
        assert_eq!(expected_md5_from_url(&url), Some(md5.to_string()));

        let sample =
            url::Url::parse("https://static1.e621.net/data/sample/01/23/preview.jpg").unwrap();
        assert_eq!(expected_md5_from_url(&sample), None);
    }

    #[test]
    fn detects_cloudflare_challenge_pages() {
        assert!(is_cloudflare_challenge("Just a moment..."));
        assert!(is_cloudflare_challenge(
            "<title>Cloudflare Error 1020</title>"
        ));
        assert!(!is_cloudflare_challenge("{\"success\":false}"));
    }

    #[test]
    fn search_request_keeps_tags_limit_and_page() {
        let http = wreq::Client::builder().build().unwrap();
        let params = search_params("cat rating:s".to_string(), Some(12345));
        let uri = http
            .get("https://e621.net/posts.json")
            .query(&params)
            .build()
            .unwrap()
            .uri()
            .to_string();
        assert!(uri.contains("tags="), "tags dropped from {uri}");
        assert!(uri.contains("limit=320"), "limit dropped from {uri}");
        assert!(uri.contains("page=b12345"), "page dropped from {uri}");
    }

    #[test]
    fn search_request_omits_page_on_first_page() {
        let params = search_params("dog".to_string(), None);
        assert_eq!(params.len(), 2);
        assert!(params.iter().all(|(k, _)| *k != "page"));
    }

    #[test]
    fn download_temp_file_removes_armed_path() {
        let path =
            std::env::temp_dir().join(format!("feline-core-download-guard-{}", std::process::id()));
        std::fs::write(&path, b"partial").unwrap();
        drop(DownloadTempFile::new(path.clone()));
        assert!(!path.exists());
    }
}
