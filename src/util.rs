use std::path::PathBuf;

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn sanitize_path_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    let trimmed = out
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    if trimmed.is_empty() {
        "_".into()
    } else {
        trimmed
    }
}

pub fn safe_truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_truncate, sanitize_path_component};

    #[test]
    fn safe_truncate_does_not_split_utf8() {
        assert_eq!(safe_truncate("가나다라마", 3), "가나다…");
        assert_eq!(safe_truncate("abc", 3), "abc");
        assert_eq!(safe_truncate("a🙂b", 2), "a🙂…");
    }

    #[test]
    fn sanitize_path_component_replaces_separators_and_controls() {
        assert_eq!(sanitize_path_component("a/b\0c"), "a_b_c");
        assert_eq!(sanitize_path_component("   ...   "), "_");
    }
}
