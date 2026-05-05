use anyhow::{Context, Result, anyhow};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use std::sync::Arc;
use std::time::Duration;

use super::ech::configure_ech_client;
use super::rate_limit::{ApiLimiter, new_api_limiter};
use super::types::{Post, PostsResponse};
use crate::config::{MediaSkip, RatingFilter, Site};
use crate::credentials::Credentials;
use crate::util::safe_truncate;

const MAX_LIMIT: u32 = 320;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct RawResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Clone)]
pub struct Client {
    api_http: reqwest::Client,
    download_http: reqwest::Client,
    limiter: Arc<ApiLimiter>,
    site: Site,
    creds: Option<Credentials>,
}

impl Client {
    pub async fn new(site: Site, creds: Option<Credentials>) -> Result<Self> {
        Self::with_limiter(site, creds, new_api_limiter(), false).await
    }

    pub async fn with_limiter(
        site: Site,
        creds: Option<Credentials>,
        limiter: Arc<ApiLimiter>,
        fail_closed_ech: bool,
    ) -> Result<Self> {
        let ua = build_user_agent(creds.as_ref());
        let mut default_headers = HeaderMap::new();
        default_headers.insert(USER_AGENT, HeaderValue::from_str(&ua)?);
        default_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let download_headers = default_headers.clone();
        let base_builder = reqwest::Client::builder()
            .default_headers(default_headers)
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15));

        let api_http = configure_ech_client(base_builder, site.host(), fail_closed_ech)
            .await?
            .build()
            .context("build API reqwest client")?;

        let download_http = reqwest::Client::builder()
            .default_headers(download_headers)
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .context("build download reqwest client")?;

        Ok(Self {
            api_http,
            download_http,
            limiter,
            site,
            creds,
        })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.download_http
    }

    pub async fn verify_login(&self) -> Result<()> {
        let creds = self
            .creds
            .as_ref()
            .ok_or_else(|| anyhow!("no credentials to verify"))?;
        if creds.is_empty() {
            return Err(anyhow!("username and API key are required"));
        }

        let url = format!("https://{}/posts.json", self.site.host());
        self.limiter.until_ready().await;

        let resp = self
            .api_http
            .get(&url)
            .query(&[("limit", "1")])
            .basic_auth(&creds.username, Some(&creds.api_key))
            .send()
            .await
            .context("send login check")?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!("invalid username or API key"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
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
        let url = format!("https://{}/posts.json", self.site.host());

        self.limiter.until_ready().await;

        let mut req = self
            .api_http
            .get(&url)
            .query(&[("tags", full_query.as_str())])
            .query(&[("limit", MAX_LIMIT.to_string().as_str())]);

        if let Some(id) = before_id {
            let page_param = format!("b{id}");
            req = req.query(&[("page", page_param.as_str())]);
        }

        if let Some(creds) = &self.creds
            && !creds.is_empty()
        {
            req = req.basic_auth(&creds.username, Some(&creds.api_key));
        }

        let resp = req.send().await.context("send search request")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
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
            other => anyhow::bail!("unsupported HTTP method {other}"),
        };

        if !query.is_empty() {
            req = req.query(query);
        }

        if let Some(creds) = &self.creds
            && !creds.is_empty()
        {
            req = req.basic_auth(&creds.username, Some(&creds.api_key));
        }

        if let Some(b) = body {
            req = req.body(b);
            if let Some(ct) = content_type {
                req = req.header(reqwest::header::CONTENT_TYPE, ct);
            }
        }

        let resp = req.send().await?;
        Ok(RawResponse {
            status: resp.status().as_u16(),
            body: resp.bytes().await?.to_vec(),
        })
    }
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
