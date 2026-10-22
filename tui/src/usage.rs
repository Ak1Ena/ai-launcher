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
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub windows: Vec<Window>,
    pub plan: Option<String>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn claude(config_dir: &Path) -> Usage {
    let Ok(text) = fs::read_to_string(config_dir.join("usage/usage.json")) else {
        return Usage::default();
    };
    claude_from(&text, now())
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
            let passed = w
                .get("resets_at")
                .and_then(Value::as_f64)
                .is_some_and(|r| r < now);
            Some(Window {
                label: label.to_string(),
                percent: if passed {
                    0
                } else {
                    pct.clamp(0.0, 100.0) as u8
                },
            })
        })
        .collect();
    Usage {
        windows,
        plan: None,
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
            })
        })
        .collect();
    Usage {
        windows,
        plan: rl
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
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
            r#"{"rate_limits":{"five_hour":{"used_percentage":34.6,"resets_at":2000},"seven_day":{"used_percentage":95,"resets_at":500}}}"#,
            1000.0,
        );
        assert_eq!(
            u.windows,
            vec![
                Window {
                    label: "5h".into(),
                    percent: 34
                },
                Window {
                    label: "7d".into(),
                    percent: 0
                }
            ]
        );
    }

    #[test]
    fn codex_reads_last_rate_limits() {
        let log = "{\"x\":1}\n{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":41.0,\"window_minutes\":300},\"plan_type\":\"plus\"}}}\n";
        let u = codex_from(log);
        assert_eq!(
            u.windows,
            vec![Window {
                label: "5h".into(),
                percent: 41
            }]
        );
        assert_eq!(u.plan.as_deref(), Some("plus"));
    }
}
