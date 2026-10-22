//! Config the bash `ai` already keeps: the providers file, Claude accounts, failover order and
//! limit marks. Same files and formats, so both front ends see the same state.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub providers: PathBuf,
    pub cla_root: PathBuf,
    pub ai_accounts: PathBuf,
    pub fo_order: PathBuf,
    pub fo_state: PathBuf,
    pub codex_home: PathBuf,
    pub cooldown: Duration,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Paths {
    pub fn from_env() -> Paths {
        let home = env_path("HOME").unwrap_or_else(|| PathBuf::from("."));
        let config = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let providers =
            env_path("AI_PROVIDERS").unwrap_or_else(|| config.join("ai-launcher/providers"));
        let state = env_path("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state"));
        let cooldown = std::env::var("AI_FO_COOLDOWN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(18000);
        Paths {
            cla_root: env_path("CLA_ROOT").unwrap_or_else(|| home.join(".claude-accounts")),
            ai_accounts: env_path("AI_ACCOUNTS").unwrap_or_else(|| home.join(".ai-accounts")),
            fo_order: env_path("AI_FO_ORDER").unwrap_or_else(|| {
                providers
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("failover")
            }),
            fo_state: env_path("AI_FO_STATE").unwrap_or_else(|| state.join("ai-launcher/failover")),
            codex_home: env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            cooldown: Duration::from_secs(cooldown),
            providers,
            home,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Provider {
    pub name: String,
    pub command: String,
    pub label: String,
}

/// The CLI a command runs: its first word after any VAR=value, without its folder.
pub fn agent(command: &str) -> String {
    command
        .split_whitespace()
        .find(|w| !is_assignment(w))
        .map(|w| w.rsplit('/').next().unwrap_or(w).to_string())
        .unwrap_or_default()
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((k, _)) => {
            !k.is_empty()
                && k.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

pub fn default_label(name: &str) -> String {
    match name {
        "claude" => "Claude Code",
        "codex" => "Codex CLI",
        "gemini" => "Gemini CLI",
        "agy" => "Antigravity",
        "kimi" => "Kimi Code",
        "opencode" => "OpenCode",
        "qwen" => "Qwen Code",
        other => other,
    }
    .to_string()
}

pub fn parse_providers(text: &str) -> Vec<Provider> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let name = f.next()?.trim();
            let command = f.next()?.trim();
            if name.is_empty() || command.is_empty() {
                return None;
            }
            let label = f
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            Some(Provider {
                name: name.into(),
                command: command.into(),
                label: label.unwrap_or_else(|| default_label(name)),
            })
        })
        .collect()
}

/// The providers file, created like the bash `ai` does on first run (every known CLI on PATH).
pub fn providers(paths: &Paths) -> Vec<Provider> {
    if let Ok(text) = fs::read_to_string(&paths.providers) {
        return parse_providers(&text);
    }
    let found: Vec<Provider> = [
        "claude", "codex", "gemini", "agy", "kimi", "opencode", "qwen",
    ]
    .iter()
    .filter(|n| on_path(n))
    .map(|n| Provider {
        name: n.to_string(),
        command: n.to_string(),
        label: default_label(n),
    })
    .collect();
    let text: String = found
        .iter()
        .map(|p| format!("{}\t{}\t{}\n", p.name, p.command, p.label))
        .collect();
    if let Some(dir) = paths.providers.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&paths.providers, text);
    found
}

pub fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Claude,
    Codex,
    Other,
}

/// Something the sidebar can start: a cla account (`claude/<name>`) or a provider.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    /// The saved command (the `claude` provider's for cla accounts).
    pub command: String,
}

impl Entry {
    pub fn in_failover(&self) -> bool {
        self.kind != Kind::Other
    }
}

/// Everything to show, in providers order: the `claude` provider expands into its cla accounts.
pub fn entries(paths: &Paths, providers: &[Provider]) -> Vec<Entry> {
    let mut out = Vec::new();
    for p in providers {
        if p.name == "claude" {
            for acct in cla_accounts(&paths.cla_root) {
                out.push(Entry {
                    id: format!("claude/{acct}"),
                    kind: Kind::Claude,
                    label: format!("Claude account {acct}"),
                    command: p.command.clone(),
                });
            }
            continue;
        }
        let kind = match agent(&p.command).as_str() {
            "claude" => Kind::Claude,
            "codex" => Kind::Codex,
            _ => Kind::Other,
        };
        out.push(Entry {
            id: p.name.clone(),
            kind,
            label: p.label.clone(),
            command: p.command.clone(),
        });
    }
    out
}

pub fn cla_accounts(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Failover members in priority order with their on/off state: the saved order first, then
/// members not saved yet (Claude before Codex), all on.
pub fn order(paths: &Paths, entries: &[Entry]) -> Vec<(String, bool)> {
    let saved = fs::read_to_string(&paths.fo_order).unwrap_or_default();
    order_from(&saved, entries)
}

pub fn order_from(saved: &str, entries: &[Entry]) -> Vec<(String, bool)> {
    let members: Vec<&Entry> = entries.iter().filter(|e| e.in_failover()).collect();
    let mut out: Vec<(String, bool)> = Vec::new();
    for line in saved.lines() {
        let mut f = line.split('\t');
        let id = f.next().unwrap_or("");
        if id.is_empty() || out.iter().any(|(m, _)| m == id) || !members.iter().any(|e| e.id == id)
        {
            continue;
        }
        out.push((id.to_string(), f.next() != Some("off")));
    }
    for kind in [Kind::Claude, Kind::Codex] {
        for e in members.iter().filter(|e| e.kind == kind) {
            if !out.iter().any(|(m, _)| *m == e.id) {
                out.push((e.id.clone(), true));
            }
        }
    }
    out
}

/// Saves the order; lines of members not present right now are kept after it.
pub fn save_order(paths: &Paths, order: &[(String, bool)]) {
    if order.is_empty() {
        return;
    }
    let saved = fs::read_to_string(&paths.fo_order).unwrap_or_default();
    let mut text: String = order
        .iter()
        .map(|(m, on)| format!("{m}\t{}\n", if *on { "on" } else { "off" }))
        .collect();
    for line in saved.lines() {
        let id = line.split('\t').next().unwrap_or("");
        if !id.is_empty() && !order.iter().any(|(m, _)| m == id) {
            text.push_str(line);
            text.push('\n');
        }
    }
    if let Some(dir) = paths.fo_order.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&paths.fo_order, text);
}

fn on_flag(paths: &Paths) -> PathBuf {
    let mut p = paths.fo_order.clone().into_os_string();
    p.push(".on");
    PathBuf::from(p)
}

pub fn failover_on(paths: &Paths) -> bool {
    on_flag(paths).exists()
}

pub fn set_failover(paths: &Paths, on: bool) {
    let flag = on_flag(paths);
    if on {
        if let Some(dir) = flag.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(flag, "");
    } else {
        let _ = fs::remove_file(flag);
    }
}

fn limit_mark(paths: &Paths, id: &str) -> PathBuf {
    paths.fo_state.join("limited").join(id)
}

/// Time left before a member that hit its limit is tried again.
// shortcut: a fixed cooldown, not the account's real reset time; read error_details if that matters.
pub fn limited_left(paths: &Paths, id: &str) -> Option<Duration> {
    let at = fs::metadata(limit_mark(paths, id))
        .and_then(|m| m.modified())
        .ok()?;
    let since = SystemTime::now().duration_since(at).unwrap_or_default();
    paths.cooldown.checked_sub(since).filter(|d| !d.is_zero())
}

pub fn mark_limited(paths: &Paths, id: &str) {
    let mark = limit_mark(paths, id);
    if let Some(dir) = mark.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(mark, "");
}

pub fn clear_limit(paths: &Paths, id: &str) {
    let _ = fs::remove_file(limit_mark(paths, id));
}

/// The config folder a Claude entry logs in with. `base` is CLAUDE_CONFIG_DIR as ai started.
pub fn claude_dir(paths: &Paths, entry: &Entry, base: Option<&Path>) -> PathBuf {
    if let Some(acct) = entry.id.strip_prefix("claude/") {
        return paths.cla_root.join(acct);
    }
    let set = entry
        .command
        .split_whitespace()
        .find_map(|w| w.strip_prefix("CLAUDE_CONFIG_DIR="))
        .map(|d| match d.strip_prefix("~/") {
            Some(rest) => paths.home.join(rest),
            None => PathBuf::from(d),
        });
    set.or_else(|| base.map(Path::to_path_buf))
        .unwrap_or_else(|| paths.home.join(".claude"))
}

/// A Codex or Gemini provider's own login folder, made by `ai add` for a 2nd login of the same CLI.
pub fn account_dir(paths: &Paths, entry: &Entry) -> Option<PathBuf> {
    let a = agent(&entry.command);
    if a != "codex" && a != "gemini" {
        return None;
    }
    let d = paths.ai_accounts.join(&a).join(&entry.id);
    d.is_dir().then_some(d)
}

// ── changing accounts (the same files `ai add/edit/rm` and `cla add/rm` change) ──

/// One word, safe as a file name: letters, digits, `.`, `_`, `-`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Writes a provider line in place (or appends it); `None` deletes it. Keeps line order.
pub fn put_provider(paths: &Paths, name: &str, value: Option<(&str, &str)>) -> std::io::Result<()> {
    let text = match fs::read_to_string(&paths.providers) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut out = String::new();
    let mut done = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if line.split('\t').next() == Some(name) {
            if let Some((cmd, label)) = value {
                out.push_str(&format!("{name}\t{cmd}\t{label}\n"));
            }
            done = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !done && let Some((cmd, label)) = value {
        out.push_str(&format!("{name}\t{cmd}\t{label}\n"));
    }
    if let Some(dir) = paths.providers.parent() {
        fs::create_dir_all(dir)?;
    }
    // temp file beside the target, then rename: a crash never leaves half a file
    let mut tmp = paths.providers.clone().into_os_string();
    tmp.push(format!(".tmp{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, out)?;
    fs::rename(&tmp, &paths.providers).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// A new cla account folder. Settings, skills, plugins and memory are shared with the main
/// account through links, the login stays separate (as `cla add`).
pub fn add_claude_account(paths: &Paths, name: &str) -> std::io::Result<PathBuf> {
    let dir = paths.cla_root.join(name);
    fs::create_dir_all(&dir)?;
    let main = paths.home.join(".claude");
    for f in [
        "settings.json",
        "settings.local.json",
        "CLAUDE.md",
        "agents",
        "skills",
        "plugins",
        "hooks",
        "commands",
        "projects",
    ] {
        let (from, to) = (main.join(f), dir.join(f));
        // symlink_metadata: a dangling link already counts as present
        if from.exists() && to.symlink_metadata().is_err() {
            #[cfg(unix)]
            match std::os::unix::fs::symlink(&from, &to) {
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
                _ => {}
            }
        }
    }
    // the accounts show up through the `claude` provider
    if !fs::read_to_string(&paths.providers)
        .unwrap_or_default()
        .lines()
        .any(|l| l.split('\t').next() == Some("claude"))
    {
        put_provider(paths, "claude", Some(("claude", "Claude Code")))?;
    }
    Ok(dir)
}

/// Moves a cla account folder to `<cla root>/.trash/` instead of erasing it (its login, history
/// and settings stay recoverable). Folders starting with `.` are not accounts.
pub fn trash_claude_account(paths: &Paths, name: &str) -> std::io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let trash = paths.cla_root.join(".trash");
    fs::create_dir_all(&trash)?;
    let from = paths.cla_root.join(name);
    // log out first (as `cla rm`); failure is fine
    let _ = std::process::Command::new("claude")
        .args(["auth", "logout"])
        .env("CLAUDE_CONFIG_DIR", &from)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let mut to = trash.join(format!("{name}-{stamp}"));
    let mut n = 2;
    while to.symlink_metadata().is_ok() {
        to = trash.join(format!("{name}-{stamp}-{n}"));
        n += 1;
    }
    fs::rename(&from, &to)?;
    Ok(to)
}

/// A 2nd provider of the same Codex/Gemini CLI gets its own login folder (as `ai add`).
/// Kept on delete, so adding the same name again gets the same login back.
pub fn add_login_dir(paths: &Paths, name: &str, command: &str) -> Option<PathBuf> {
    let a = agent(command);
    if a != "codex" && a != "gemini" {
        return None;
    }
    let others = providers(paths);
    let shares = others.iter().any(|p| {
        p.name != name
            && agent(&p.command) == a
            && !paths.ai_accounts.join(&a).join(&p.name).is_dir()
    });
    if !shares {
        return None;
    }
    let d = paths.ai_accounts.join(&a).join(name);
    fs::create_dir_all(&d).ok()?;
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, kind: Kind) -> Entry {
        Entry {
            id: id.into(),
            kind,
            label: id.into(),
            command: String::new(),
        }
    }

    #[test]
    fn agent_skips_env_and_folders() {
        assert_eq!(agent("FOO=1 /usr/bin/codex --x"), "codex");
        assert_eq!(agent("claude"), "claude");
    }

    #[test]
    fn providers_parse_with_default_label() {
        let p = parse_providers("codex\tcodex\n\nbad\nx\ty\tWhy\n");
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].label, "Codex CLI");
        assert_eq!(p[1].label, "Why");
    }

    #[test]
    fn order_keeps_saved_then_adds_claude_before_codex() {
        let es = [
            e("cx", Kind::Codex),
            e("claude/a", Kind::Claude),
            e("g", Kind::Other),
            e("n", Kind::Claude),
        ];
        let o = order_from("n\toff\ngone\ton\n", &es);
        assert_eq!(
            o,
            vec![
                ("n".into(), false),
                ("claude/a".into(), true),
                ("cx".into(), true)
            ]
        );
    }

    fn tmp_paths(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("ai-cfg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home/.claude")).unwrap();
        fs::write(root.join("home/.claude/settings.json"), "{}").unwrap();
        Paths {
            home: root.join("home"),
            providers: root.join("cfg/providers"),
            cla_root: root.join("acc"),
            ai_accounts: root.join("ai-acc"),
            fo_order: root.join("cfg/failover"),
            fo_state: root.join("state"),
            codex_home: root.join("home/.codex"),
            cooldown: Duration::from_secs(60),
        }
    }

    #[test]
    fn put_provider_edits_in_place_and_deletes() {
        let p = tmp_paths("put");
        put_provider(&p, "a", Some(("a", "A"))).unwrap();
        put_provider(&p, "b", Some(("b", "B"))).unwrap();
        put_provider(&p, "a", Some(("a --x", "A2"))).unwrap();
        assert_eq!(
            fs::read_to_string(&p.providers).unwrap(),
            "a\ta --x\tA2\nb\tb\tB\n"
        );
        put_provider(&p, "a", None).unwrap();
        assert_eq!(fs::read_to_string(&p.providers).unwrap(), "b\tb\tB\n");
    }

    #[test]
    fn put_provider_refuses_unreadable_file() {
        let p = tmp_paths("bad");
        fs::create_dir_all(p.providers.parent().unwrap()).unwrap();
        fs::write(&p.providers, [0xff, 0xfe]).unwrap();
        assert!(put_provider(&p, "a", Some(("a", "A"))).is_err());
        assert_eq!(fs::read(&p.providers).unwrap(), vec![0xff, 0xfe]);
    }

    #[cfg(unix)]
    #[test]
    fn add_account_retries_over_dangling_link_and_trash_names_are_unique() {
        let p = tmp_paths("retry");
        let d = p.cla_root.join("w");
        fs::create_dir_all(&d).unwrap();
        std::os::unix::fs::symlink("/nonexistent/x", d.join("settings.json")).unwrap();
        add_claude_account(&p, "w").unwrap();
        add_claude_account(&p, "w").unwrap();
        let a = trash_claude_account(&p, "w").unwrap();
        fs::create_dir_all(p.cla_root.join("w")).unwrap();
        let b = trash_claude_account(&p, "w").unwrap();
        assert_ne!(a, b);
        assert!(a.exists() && b.exists());
    }

    #[test]
    fn claude_account_add_and_trash() {
        let p = tmp_paths("cla");
        let d = add_claude_account(&p, "work").unwrap();
        assert!(d.join("settings.json").exists());
        assert!(
            fs::read_to_string(&p.providers)
                .unwrap()
                .starts_with("claude\t")
        );
        assert_eq!(cla_accounts(&p.cla_root), vec!["work".to_string()]);
        trash_claude_account(&p, "work").unwrap();
        assert!(
            cla_accounts(&p.cla_root).is_empty(),
            "trash is not an account"
        );
    }

    #[test]
    fn second_codex_gets_its_own_login() {
        let p = tmp_paths("login");
        put_provider(&p, "codex", Some(("codex", "Codex"))).unwrap();
        assert!(add_login_dir(&p, "codex", "codex").is_none());
        put_provider(&p, "work", Some(("codex", "Codex work"))).unwrap();
        assert_eq!(
            add_login_dir(&p, "work", "codex"),
            Some(p.ai_accounts.join("codex/work"))
        );
    }

    #[test]
    fn names_are_path_safe() {
        assert!(valid_name("work-2.a"));
        assert!(
            !valid_name("..") && !valid_name("a/b") && !valid_name(".trash") && !valid_name("")
        );
    }
}
