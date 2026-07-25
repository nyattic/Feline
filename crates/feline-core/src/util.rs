use std::io::Write;
use std::path::{Path, PathBuf};

pub fn write_file_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn migrate_legacy_file(path: &Path, filename: &str) {
    if cfg!(target_os = "macos") || path.exists() {
        return;
    }
    let legacy = exe_dir().join(filename);
    if legacy == path || !legacy.is_file() {
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    match std::fs::copy(&legacy, path) {
        Ok(_) => tracing::info!(
            "migrated legacy data file from `{}` to `{}`",
            legacy.display(),
            path.display()
        ),
        Err(err) => tracing::warn!(
            "failed to migrate legacy data file from `{}`: {err}",
            legacy.display()
        ),
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn home_dir() -> PathBuf {
    let key = if cfg!(target_os = "windows") {
        "USERPROFILE"
    } else {
        "HOME"
    };
    std::env::var_os(key)
        .map(PathBuf::from)
        .unwrap_or_else(exe_dir)
}

#[cfg(target_os = "macos")]
pub fn config_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Feline")
}

#[cfg(target_os = "windows")]
pub fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join("AppData/Roaming"))
        .join("Feline")
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
        .join("Feline")
}

pub fn state_dir() -> PathBuf {
    config_dir()
}

#[cfg(target_os = "macos")]
pub fn log_dir() -> PathBuf {
    home_dir().join("Library/Logs/Feline")
}

#[cfg(target_os = "windows")]
pub fn log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join("AppData/Local"))
        .join("Feline/log")
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub fn log_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/state"))
        .join("Feline/log")
}

pub fn default_download_dir() -> PathBuf {
    home_dir().join("Downloads/Feline")
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
