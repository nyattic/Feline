use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct PostsResponse {
    pub posts: Vec<Post>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Post {
    pub id: u64,
    pub file: PostFile,
    pub tags: PostTags,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostFile {
    pub ext: String,
    pub size: u64,
    pub md5: String,
    /// `url` may be null for e.g. deleted posts or rating restrictions.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PostTags {
    #[serde(default)]
    pub artist: Vec<String>,
}

impl Post {
    /// Returns the first artist tag or "unknown_artist" if none.
    pub fn primary_artist(&self) -> &str {
        self.tags
            .artist
            .first()
            .map(String::as_str)
            .unwrap_or("unknown_artist")
    }
}
