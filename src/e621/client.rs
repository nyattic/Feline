use anyhow::{Context, Result, anyhow};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use std::sync::Arc;
use std::time::Duration;

use super::rate_limit::{ApiLimiter, new_api_limiter};
use super::types::{Post, PostsResponse};
use crate::config::{MediaSkip, RatingFilter, Site};
use crate::credentials::Credentials;
use crate::util::safe_truncate;

const MAX_LIMIT: u32 = 320;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    limiter: Arc<ApiLimiter>,
    site: Site,
    creds: Option<Credentials>,
}

impl Client {
    pub fn new(site: Site, creds: Option<Credentials>) -> Result<Self> {
        Self::with_limiter(site, creds, new_api_limiter())
    }

    pub fn with_limiter(
        site: Site,
        creds: Option<Credentials>,
        limiter: Arc<ApiLimiter>,
    ) -> Result<Self> {
        let ua = build_user_agent(creds.as_ref());
        let mut default_headers = HeaderMap::new();
        default_headers.insert(USER_AGENT, HeaderValue::from_str(&ua)?);
        default_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .default_headers(default_headers)
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .context("build reqwest client")?;

        Ok(Self {
            http,
            limiter,
            site,
            creds,
        })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Hit an authenticated endpoint to confirm the provided credentials work.
    /// Returns Ok(()) on 2xx, Err with a readable message on 401/other failure.
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
            .http
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

    /// Search a single page of posts. `before_id` paginates backwards through
    /// the result set using e621's `page=b{id}` form, which scales past 750 deep.
    /// When `before_id` is None, returns the newest page.
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
            .http
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
        // Negate if not already negated.
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
