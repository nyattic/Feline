use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Site {
    #[default]
    E621,
    E926,
}

pub const SITES: [Site; 2] = [Site::E621, Site::E926];

impl Site {
    pub fn host(&self) -> &'static str {
        match self {
            Site::E621 => "e621.net",
            Site::E926 => "e926.net",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Site::E621 => "e621",
            Site::E926 => "e926",
        }
    }

    pub fn media_hosts(&self) -> &'static [&'static str] {
        match self {
            Site::E621 | Site::E926 => &[
                "e621.net",
                "static1.e621.net",
                "static2.e621.net",
                "e926.net",
                "static1.e926.net",
                "static2.e926.net",
            ],
        }
    }

    pub fn credential_domains(&self) -> &'static [&'static str] {
        match self {
            Site::E621 | Site::E926 => &["e621.net", "e926.net"],
        }
    }

    pub fn keychain_account(&self) -> &'static str {
        match self {
            Site::E621 | Site::E926 => "e621",
        }
    }

    pub fn from_media_host(host: &str) -> Option<Site> {
        let host = host.trim_end_matches('.');
        for site in SITES {
            if site
                .media_hosts()
                .iter()
                .any(|h| host == *h || host.ends_with(&format!(".{h}")))
            {
                return Some(site);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RatingFilter {
    pub safe: bool,
    pub questionable: bool,
    pub explicit: bool,
}

impl RatingFilter {
    pub fn all() -> Self {
        Self {
            safe: true,
            questionable: true,
            explicit: true,
        }
    }

    pub fn as_query_fragment(&self) -> Option<String> {
        let selected: Vec<&'static str> = [
            self.safe.then_some("s"),
            self.questionable.then_some("q"),
            self.explicit.then_some("e"),
        ]
        .into_iter()
        .flatten()
        .collect();

        if selected.is_empty() || selected.len() == 3 {
            return None;
        }

        if selected.len() == 1 {
            Some(format!("rating:{}", selected[0]))
        } else {
            Some(format!("rating:{}", selected.join(",")))
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MediaSkip {
    #[serde(default)]
    pub video: bool,
    #[serde(default)]
    pub flash: bool,
    #[serde(default)]
    pub animation: bool,
}

impl MediaSkip {
    pub fn as_query_tokens(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.video {
            out.push("-type:webm");
        }
        if self.flash {
            out.push("-type:swf");
        }
        if self.animation {
            out.push("-type:gif");
        }
        out
    }
}
