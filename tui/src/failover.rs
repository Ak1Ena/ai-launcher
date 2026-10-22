//! Failover pieces that don't draw: what to pass a relaunched claude, the Codex handoff, moving
//! the transcript to the next account, and which member comes next.

use crate::config::{self, Entry, Paths};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub const CONTINUE: &str = "The previous account hit its usage limit and this session moved to a new account. Continue the work exactly where you left off.";

/// The mod's files, written next to the state so an installed binary carries its own mod.
const MOD_FILES: [(&str, &str); 3] = [
    (
        ".claude-plugin/plugin.json",
        include_str!("../../mods/failover/.claude-plugin/plugin.json"),
    ),
    (
        "hooks/hooks.json",
        include_str!("../../mods/failover/hooks/hooks.json"),
    ),
    (
        "hooks/failover.ts",
        include_str!("../../mods/failover/hooks/failover.ts"),
    ),
];

pub fn install_mod(paths: &Paths) -> PathBuf {
    let dir = paths.fo_state.join("mod");
    for (name, text) in MOD_FILES {
        let f = dir.join(name);
        if fs::read_to_string(&f).ok().as_deref() != Some(text) {
            if let Some(d) = f.parent() {
                let _ = fs::create_dir_all(d);
            }
            let _ = fs::write(&f, text);
        }
    }
    dir
}

/// User args split into flags kept on every relaunch and the prompt, which is dropped on a relaunch.
#[derive(Debug, Default, PartialEq)]
pub struct Split {
    pub keep: Vec<String>,
    pub prompt: Option<String>,
}

/// `value_flags`: flags that take a value, as `claude --help` lists them.
pub fn split_args(args: &[String], value_flags: &[String]) -> Split {
    let mut s = Split::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--resume" | "-r" => {
                if args.get(i + 1).is_some_and(|n| !n.starts_with('-')) {
                    i += 1;
                }
            }
            "--session-id" => i += 1,
            "--continue" | "-c" => {}
            _ if a.starts_with("--resume=") || a.starts_with("--session-id=") => {}
            "--" => {
                s.prompt = Some(args[i + 1..].join(" "));
                break;
            }
            _ if a.starts_with('-') => {
                s.keep.push(a.clone());
                if !a.contains('=') && value_flags.iter().any(|f| f == a) && i + 1 < args.len() {
                    i += 1;
                    s.keep.push(args[i].clone());
                }
            }
            _ => s.prompt = Some(a.clone()),
        }
        i += 1;
    }
    s
}

/// Flags that take a value: from `claude --help` (`--model <model>`, `-n, --name <name>`,
/// `--allowedTools, --allowed-tools <tools...>`), or common ones when claude isn't on PATH.
pub fn value_flags() -> Vec<String> {
    let help = std::process::Command::new("claude")
        .arg("--help")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let flags = parse_value_flags(&help);
    if !flags.is_empty() {
        return flags;
    }
    [
        "--model",
        "--settings",
        "--add-dir",
        "--permission-mode",
        "--mcp-config",
        "--agent",
        "--effort",
        "--name",
        "-n",
        "--append-system-prompt",
        "--system-prompt",
        "--allowedTools",
        "--allowed-tools",
        "--disallowedTools",
        "--disallowed-tools",
        "--fallback-model",
        "--output-format",
        "--input-format",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn parse_value_flags(help: &str) -> Vec<String> {
    help.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            if !l.starts_with('-') {
                return None;
            }
            let (names, rest) = l.split_once(" <")?;
            let _ = rest;
            Some(names.split(", ").map(str::to_string).collect::<Vec<_>>())
        })
        .flatten()
        .filter(|n| n.starts_with('-') && !n.contains(' '))
        .collect()
}

/// What the mod wrote when a turn stopped on a rate limit.
pub struct Handoff {
    pub session_id: Option<String>,
    pub transcript: Option<PathBuf>,
}

pub fn read_handoff(path: &Path) -> Option<Handoff> {
    let v: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let transcript = s("transcript_path")
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    let session_id = transcript.as_ref().and(s("session_id"));
    Some(Handoff {
        session_id,
        transcript,
    })
}

/// Next member to try: on, not tried in this run, not cooling down.
pub fn next_member(paths: &Paths, entries: &[Entry], tried: &[String]) -> Option<String> {
    config::order(paths, entries)
        .into_iter()
        .find(|(m, on)| *on && !tried.contains(m) && config::limited_left(paths, m).is_none())
        .map(|(m, _)| m)
}

/// Copies the session into the next account's config folder so `--resume` finds it there,
/// with the `<id>/` folder beside it (subagent and tool files).
pub fn move_transcript(transcript: &Path, config_dir: &Path) -> std::io::Result<()> {
    let project = transcript
        .parent()
        .and_then(Path::file_name)
        .ok_or(std::io::ErrorKind::NotFound)?;
    let dest = config_dir.join("projects").join(project);
    fs::create_dir_all(&dest)?;
    let target = dest.join(transcript.file_name().ok_or(std::io::ErrorKind::NotFound)?);
    if fs::canonicalize(&target).ok() == fs::canonicalize(transcript).ok() {
        return Ok(());
    }
    fs::copy(transcript, &target)?;
    let side = transcript.with_extension("");
    if side.is_dir() {
        copy_dir(&side, &dest.join(side.file_name().unwrap()))?;
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &t)?
        } else {
            fs::copy(e.path(), t).map(|_| ())?
        }
    }
    Ok(())
}

/// The main conversation of a Claude transcript as markdown, its last 60 KB (one argument must fit).
pub fn handoff_text(transcript: &Path) -> String {
    let text = fs::read_to_string(transcript).unwrap_or_default();
    let mut out = String::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        if !matches!(kind, "user" | "assistant")
            || flag("isMeta")
            || flag("isApiErrorMessage")
            || flag("isSidechain")
        {
            continue;
        }
        let body = match v.pointer("/message/content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !body.is_empty() {
            out.push_str(&format!("### {kind}\n{body}\n\n"));
        }
    }
    const MAX: usize = 60_000;
    if out.len() > MAX {
        let mut cut = out.len() - MAX;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        // start at a whole line, not the one the cut split
        let start = out[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
        out = out[start..].to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn split_keeps_flags_and_drops_session_pickers() {
        let s = split_args(
            &v(&["-p", "fix it", "--model", "opus", "--resume", "abc", "-c"]),
            &v(&["--model"]),
        );
        assert_eq!(
            s,
            Split {
                keep: v(&["-p", "--model", "opus"]),
                prompt: Some("fix it".into())
            }
        );
    }

    #[test]
    fn help_lists_value_flags_with_aliases() {
        let help = "  --model <model>  x\n  -n, --name <name>\n  --allowedTools, --allowed-tools <tools...>\n  -p, --print  y\n  -r, --resume [value]\n";
        assert_eq!(
            parse_value_flags(help),
            v(&[
                "--model",
                "-n",
                "--name",
                "--allowedTools",
                "--allowed-tools"
            ])
        );
    }

    #[test]
    fn handoff_skips_meta_and_errors() {
        let dir = std::env::temp_dir().join(format!("ai-fo-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.jsonl");
        fs::write(&f, concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"build it\"}}\n",
            "{\"type\":\"user\",\"isMeta\":true,\"message\":{\"content\":\"meta\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"half done\"}]}}\n",
            "{\"type\":\"assistant\",\"isApiErrorMessage\":true,\"message\":{\"content\":\"limit\"}}\n",
        )).unwrap();
        assert_eq!(
            handoff_text(&f),
            "### user\nbuild it\n\n### assistant\nhalf done\n\n"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
