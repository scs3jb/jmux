//! Browser profile import — cookies from Firefox and Chrome/Chromium.
//!
//! Uses the `sqlite3` CLI tool to read Firefox's cookies.sqlite without
//! adding a Rust SQLite dependency. Must be called on the GTK main thread.


/// A parsed cookie ready to inject into WebKit.
#[derive(Debug)]
pub struct ImportedCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expiry: i64,
    pub is_secure: bool,
    pub is_http_only: bool,
}

/// Source browser for import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    Firefox,
    Chrome,
    Chromium,
}

/// Find the most-recently-used Firefox profile directory.
fn find_firefox_cookies_db() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    let ff_dir = home.join(".mozilla/firefox");
    if !ff_dir.exists() {
        return None;
    }
    // Parse profiles.ini to find the default profile
    let ini_path = ff_dir.join("profiles.ini");
    let ini = std::fs::read_to_string(&ini_path).ok()?;
    let mut profile_path: Option<String> = None;
    let mut is_default = false;
    let mut current_path: Option<String> = None;
    for line in ini.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if is_default {
                if let Some(ref p) = current_path {
                    profile_path = Some(p.clone());
                }
            }
            is_default = false;
            current_path = None;
        } else if let Some(rest) = line.strip_prefix("Default=") {
            is_default = rest.trim() == "1";
        } else if let Some(rest) = line.strip_prefix("Path=") {
            let p = rest.trim();
            if p.starts_with('/') {
                current_path = Some(p.to_string());
            } else {
                current_path = Some(ff_dir.join(p).to_string_lossy().to_string());
            }
        }
    }
    // Handle last section
    if is_default {
        if let Some(ref p) = current_path {
            profile_path = Some(p.clone());
        }
    }
    // Fall back to first profile found by glob
    if profile_path.is_none() {
        for entry in std::fs::read_dir(&ff_dir).ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains('.') {
                let candidate = entry.path().join("cookies.sqlite");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        return None;
    }
    let cookies_db = std::path::PathBuf::from(profile_path?).join("cookies.sqlite");
    cookies_db.exists().then_some(cookies_db)
}

/// Find Chrome/Chromium cookies database.
fn find_chrome_cookies_db(source: ImportSource) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    let candidates: &[&str] = match source {
        ImportSource::Chrome => &[
            ".config/google-chrome/Default/Cookies",
            ".config/google-chrome/Profile 1/Cookies",
        ],
        ImportSource::Chromium => &[
            ".config/chromium/Default/Cookies",
            ".config/chromium/Profile 1/Cookies",
        ],
        ImportSource::Firefox => return None,
    };
    candidates
        .iter()
        .map(|p| home.join(p))
        .find(|p| p.exists())
}

/// Read Firefox cookies using the `sqlite3` CLI tool.
/// Returns an error string if sqlite3 is not found or the DB cannot be read.
pub fn read_firefox_cookies() -> Result<Vec<ImportedCookie>, String> {
    let db_path = find_firefox_cookies_db()
        .ok_or_else(|| "Firefox profile not found at ~/.mozilla/firefox/".to_string())?;

    let output = std::process::Command::new("sqlite3")
        .arg("-separator")
        .arg("\t")
        .arg(&db_path)
        .arg("SELECT host, name, value, path, expiry, isSecure, isHttpOnly FROM moz_cookies;")
        .output()
        .map_err(|e| format!("sqlite3 not found: {e}. Install sqlite3 to use this feature."))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("sqlite3 error: {stderr}"));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut cookies = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 7 {
            continue;
        }
        cookies.push(ImportedCookie {
            domain: parts[0].to_string(),
            name: parts[1].to_string(),
            value: parts[2].to_string(),
            path: parts[3].to_string(),
            expiry: parts[4].parse().unwrap_or(0),
            is_secure: parts[5] == "1",
            is_http_only: parts[6] == "1",
        });
    }
    Ok(cookies)
}

/// Read Chrome cookies using sqlite3 (only unencrypted values are imported;
/// Chrome on Linux encrypts cookies using the system keyring, so `encrypted_value`
/// rows are skipped).
pub fn read_chrome_cookies(source: ImportSource) -> Result<Vec<ImportedCookie>, String> {
    let db_path = find_chrome_cookies_db(source).ok_or_else(|| {
        let name = if source == ImportSource::Chrome { "Chrome" } else { "Chromium" };
        format!("{name} profile not found")
    })?;

    let output = std::process::Command::new("sqlite3")
        .arg("-separator")
        .arg("\t")
        .arg(&db_path)
        .arg(
            "SELECT host_key, name, value, path, expires_utc/1000000-11644473600, \
             is_secure, is_httponly \
             FROM cookies WHERE length(value) > 0;",
        )
        .output()
        .map_err(|e| format!("sqlite3 not found: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("sqlite3 error: {stderr}"));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut cookies = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 7 {
            continue;
        }
        cookies.push(ImportedCookie {
            domain: parts[0].to_string(),
            name: parts[1].to_string(),
            value: parts[2].to_string(),
            path: parts[3].to_string(),
            expiry: parts[4].parse().unwrap_or(0),
            is_secure: parts[5] == "1",
            is_http_only: parts[6] == "1",
        });
    }
    Ok(cookies)
}

/// Inject cookies into the WebKit NetworkSession's CookieManager.
/// Must be called on the GTK main thread.
#[cfg(feature = "webkit")]
pub fn inject_cookies_into_webkit(cookies: Vec<ImportedCookie>) -> usize {
    let session = crate::ui::browser_panel::shared_network_session();
    let Some(mgr) = session.cookie_manager() else {
        return 0;
    };
    let mut count = 0;
    for cookie in cookies {
        // max_age -1 means session cookie, positive = seconds until expiry
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let max_age = if cookie.expiry <= 0 {
            -1
        } else {
            (cookie.expiry - now).max(0) as i32
        };
        if max_age == 0 && cookie.expiry > 0 {
            continue; // already expired
        }
        let mut sc = soup::Cookie::new(&cookie.name, &cookie.value, &cookie.domain, &cookie.path, max_age);
        sc.set_secure(cookie.is_secure);
        sc.set_http_only(cookie.is_http_only);
        mgr.add_cookie(&sc, gio::Cancellable::NONE, |_: Result<(), _>| {});
        count += 1;
    }
    count
}

/// High-level import: reads cookies from the given source and injects them
/// into WebKit. Returns `(imported_count, error_message)`.
#[cfg(feature = "webkit")]
pub fn import_from(source: ImportSource) -> (usize, Option<String>) {
    let result = match source {
        ImportSource::Firefox => read_firefox_cookies(),
        ImportSource::Chrome => read_chrome_cookies(ImportSource::Chrome),
        ImportSource::Chromium => read_chrome_cookies(ImportSource::Chromium),
    };
    match result {
        Ok(cookies) => {
            let count = inject_cookies_into_webkit(cookies);
            (count, None)
        }
        Err(e) => (0, Some(e)),
    }
}
