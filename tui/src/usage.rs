//! Plan usage from files the agents already write (no network): Claude's status line saves
//! `<config>/usage/usage.json`, Codex keeps rate limits in its newest session log.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub label: String,
    pub percent: u8,
    /// when the window resets (unix seconds); none once it has passed
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub windows: Vec<Window>,
    pub plan: Option<String>,
    /// when the agent last saved this usage (unix seconds)
    pub saved_at: Option<i64>,
    /// the Claude account's login email
    pub email: Option<String>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn claude(config_dir: &Path) -> Usage {
    let mut u = fs::read_to_string(config_dir.join("usage/usage.json"))
        .map(|t| claude_from(&t, now()))
        .unwrap_or_default();
    // Claude Code keeps .claude.json inside CLAUDE_CONFIG_DIR, or beside ~/.claude without it
    let json = if config_dir.ends_with(".claude") {
        config_dir.with_file_name(".claude.json")
    } else {
        config_dir.join(".claude.json")
    };
    u.email = fs::read_to_string(json).ok().and_then(|t| email_from(&t));
    u
}

pub fn email_from(text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(
        v.get("oauthAccount")?
            .get("emailAddress")?
            .as_str()?
            .to_string(),
    )
}

/// " · 90m old" style note for usage saved 10 minutes ago or more, like the bash picker.
pub fn age_note(saved_at: i64, now: i64) -> Option<String> {
    let m = (now - saved_at) / 60;
    let n = match m {
        m if m >= 2880 => format!("{}d", m / 1440),
        m if m >= 120 => format!("{}h", m / 60),
        m => format!("{m}m"),
    };
    (m >= 10).then(|| format!("{n} old"))
}

/// Local "Tue 12:59" for a unix time, like the bash picker's `strftime("%a %H:%M")`.
pub fn local_hm(t: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    // SAFETY: tm is plain data that localtime fills in
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = t as libc::time_t;
    #[cfg(unix)]
    let ok = !unsafe { libc::localtime_r(&tt, &mut tm) }.is_null();
    #[cfg(windows)]
    let ok = unsafe { libc::localtime_s(&mut tm, &tt) } == 0;
    if !ok {
        return String::new();
    }
    format!(
        "{} {:02}:{:02}",
        DAYS[tm.tm_wday.rem_euclid(7) as usize],
        tm.tm_hour,
        tm.tm_min
    )
}

pub fn now_secs() -> i64 {
    now() as i64
}

pub fn claude_from(text: &str, now: f64) -> Usage {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Usage::default();
    };
    let windows = [("five_hour", "5h"), ("seven_day", "7d")]
        .iter()
        .filter_map(|(key, label)| {
            let w = v.get("rate_limits")?.get(*key)?;
            let pct = w.get("used_percentage")?.as_f64()?;
            // a window whose reset time has passed is empty again
            let at = w.get("resets_at").and_then(Value::as_f64);
            let passed = at.is_some_and(|r| r < now);
            Some(Window {
                label: label.to_string(),
                percent: if passed {
                    0
                } else {
                    pct.clamp(0.0, 100.0) as u8
                },
                resets_at: at.filter(|_| !passed).map(|r| r as i64),
            })
        })
        .collect();
    Usage {
        windows,
        saved_at: v.get("saved_at").and_then(Value::as_f64).map(|t| t as i64),
        ..Usage::default()
    }
}

/// Newest file under `sessions/YYYY/MM/DD/`, where name order is time order.
fn newest_session(home: &Path) -> Option<PathBuf> {
    let mut dir = home.join("sessions");
    for _ in 0..3 {
        dir = newest_entry(&dir, true)?;
    }
    newest_entry(&dir, false)
}

fn newest_entry(dir: &Path, want_dir: bool) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            if want_dir {
                p.is_dir()
            } else {
                p.extension().is_some_and(|x| x == "jsonl")
            }
        })
        .max()
}

pub fn codex(home: &Path) -> Usage {
    let Some(text) = newest_session(home).and_then(|f| fs::read_to_string(f).ok()) else {
        return Usage::default();
    };
    codex_from(&text)
}

pub fn codex_from(text: &str) -> Usage {
    let Some(rl) = text
        .lines()
        .rev()
        .filter(|l| l.contains("\"rate_limits\""))
        .find_map(|l| {
            serde_json::from_str::<Value>(l)
                .ok()
                .and_then(|v| find_key(&v, "rate_limits").cloned())
        })
    else {
        return Usage::default();
    };
    let windows = ["primary", "secondary"]
        .iter()
        .filter_map(|k| {
            let w = rl.get(*k).filter(|w| !w.is_null())?;
            let minutes = w.get("window_minutes").and_then(Value::as_u64).unwrap_or(0);
            let label = match minutes {
                m if m > 0 && m % 1440 == 0 => format!("{}d", m / 1440),
                m if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
                m => format!("{m}m"),
            };
            Some(Window {
                label,
                percent: w.get("used_percent")?.as_f64()?.clamp(0.0, 100.0) as u8,
                resets_at: w.get("resets_at").and_then(Value::as_f64).map(|t| t as i64),
            })
        })
        .collect();
    Usage {
        windows,
        plan: rl
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..Usage::default()
    }
}

fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m
            .get(key)
            .or_else(|| m.values().find_map(|x| find_key(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find_key(x, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_windows_and_passed_reset() {
        let u = claude_from(
            r#"{"rate_limits":{"five_hour":{"used_percentage":34.6,"resets_at":2000},"seven_day":{"used_percentage":95,"resets_at":500}},"saved_at":900}"#,
            1000.0,
        );
        assert_eq!(
            u.windows,
            vec![
                Window {
                    label: "5h".into(),
                    percent: 34,
                    resets_at: Some(2000)
                },
                Window {
                    label: "7d".into(),
                    percent: 0,
                    resets_at: None
                }
            ]
        );
        assert_eq!(u.saved_at, Some(900));
    }

    #[test]
    fn age_note_matches_bash() {
        assert_eq!(age_note(1000, 1000 + 9 * 60), None);
        assert_eq!(age_note(0, 90 * 60).as_deref(), Some("90m old"));
        assert_eq!(age_note(0, 3 * 3600).as_deref(), Some("3h old"));
        assert_eq!(age_note(0, 3 * 86400).as_deref(), Some("3d old"));
    }

    #[test]
    fn email_and_local_time() {
        assert_eq!(
            email_from(r#"{"oauthAccount":{"emailAddress":"a@example.com"}}"#).as_deref(),
            Some("a@example.com")
        );
        assert_eq!(email_from("{}"), None);
        let t = local_hm(1_700_000_000);
        assert_eq!(t.len(), 9, "{t}");
        assert!(["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].contains(&&t[..3]));
    }

    #[test]
    fn codex_reads_last_rate_limits() {
        let log = "{\"x\":1}\n{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":41.0,\"window_minutes\":300,\"resets_at\":1700000000},\"plan_type\":\"plus\"}}}\n";
        let u = codex_from(log);
        assert_eq!(
            u.windows,
            vec![Window {
                label: "5h".into(),
                percent: 41,
                resets_at: Some(1_700_000_000)
            }]
        );
        assert_eq!(u.plan.as_deref(), Some("plus"));
    }
}
