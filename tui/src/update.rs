//! Background "newer version on npm" check. Never blocks, prints or panics.
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_AGE_SECS: u64 = 24 * 3600;

fn parse(v: &str) -> Option<([u64; 3], bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, _)) => (c, true),
        None => (v, false),
    };
    let mut it = core.split('.');
    let mut n = [0u64; 3];
    for slot in &mut n {
        *slot = it.next()?.parse().ok()?;
    }
    it.next().is_none().then_some((n, pre))
}

/// True when `latest` is a strictly newer release than `current`; garbage is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        // prerelease sorts below the release (false < true is reversed, so flip)
        (Some((a, ap)), Some((b, bp))) => (a, !ap) > (b, !bp),
        _ => false,
    }
}

/// Cache line `<unix_secs>\t<version>` -> version, if still fresh at `now`.
fn fresh_cached(text: &str, now: u64) -> Option<String> {
    let (t, v) = text.trim().split_once('\t')?;
    let t: u64 = t.parse().ok()?;
    (now.saturating_sub(t) < MAX_AGE_SECS && parse(v).is_some()).then(|| v.trim().to_string())
}

fn npm_latest() -> Option<String> {
    let out = Command::new("npm")
        .args(["view", "ai-launcher", "version"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && parse(&v).is_some()).then_some(v)
}

pub fn spawn_check(state_dir: &Path) -> Receiver<String> {
    let (tx, rx) = channel();
    if std::env::var_os("AI_LAUNCHER_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty()) {
        return rx;
    }
    let cache = state_dir.join("update-check");
    let dir = state_dir.to_path_buf();
    std::thread::spawn(move || {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let cached = std::fs::read_to_string(&cache)
            .ok()
            .and_then(|t| fresh_cached(&t, now));
        let latest = cached.or_else(|| {
            let v = npm_latest()?;
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&cache, format!("{now}\t{v}"));
            Some(v)
        });
        if let Some(v) = latest.filter(|v| is_newer(v, env!("CARGO_PKG_VERSION"))) {
            let _ = tx.send(v.trim_start_matches('v').to_string());
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("garbage", "1.0.0"));
        assert!(!is_newer("1.0.0", "x"));
        assert!(!is_newer("1.0", "0.1.0"));
    }

    #[test]
    fn cache() {
        assert_eq!(
            fresh_cached("1000\t0.2.0\n", 1500).as_deref(),
            Some("0.2.0")
        );
        assert_eq!(fresh_cached("1000\t0.2.0", 1000 + MAX_AGE_SECS), None);
        assert_eq!(fresh_cached("junk", 1), None);
        assert_eq!(fresh_cached("1\tnope", 2), None);
    }
}
