//! User plugins: folders in `Paths::plugins` with an executable `plugin` that speaks the line
//! protocol bash `ai` speaks too (lib/plugins.sh). Same files and steps on both sides:
//! `<plugin>/.off` turns one off, `<plugin>/.source` keeps the git URL it came from.

use crate::config::{self, Paths};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ROWS_TIMEOUT: Duration = Duration::from_secs(2);
/// A plugin's first `rows`/`actions` in a session: macOS scans a newly written script on its
/// first run, which can take seconds right after an install.
pub const FIRST_TIMEOUT: Duration = Duration::from_secs(10);
pub const DO_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// The bundled cliproxy plugin, shipped inside the binary (npm has no repo): file, text, executable.
const CLIPROXY: [(&str, &str, bool); 2] = [
    (
        "plugin",
        include_str!("../../plugins-extra/cliproxy/plugin"),
        true,
    ),
    (
        "config.example",
        include_str!("../../plugins-extra/cliproxy/config.example"),
        false,
    ),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Plugin {
    pub name: String,
    pub dir: PathBuf,
    pub on: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PEntry {
    pub id: String,
    pub label: String,
    pub status: String,
    /// `  <TAB><text>` lines after the entry, shown indented under it
    pub details: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    /// the `section` title; empty when the plugin gave none
    pub title: String,
    pub entries: Vec<PEntry>,
}

/// A plugin (folder) name: letters, digits, `_`, `-`.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// An entry id: letters, digits, `.`, `_`, `-`. Only ever passed as an argument.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `plugin:<name>/<id>` → (name, Some(id)); `plugin:<name>` (a section's state line) → (name, None).
pub fn split_id(id: &str) -> Option<(&str, Option<&str>)> {
    let rest = id.strip_prefix("plugin:")?;
    match rest.split_once('/') {
        Some((n, e)) => Some((n, Some(e))),
        None => Some((rest, None)),
    }
}

/// Installed plugins (on and off) by name; folders with a bad name or no `plugin` are skipped.
pub fn list(paths: &Paths) -> Vec<Plugin> {
    let mut out: Vec<Plugin> = std::fs::read_dir(&paths.plugins)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let dir = e.path();
                    (valid_name(&name) && dir.join("plugin").is_file()).then(|| Plugin {
                        on: !dir.join(".off").exists(),
                        name,
                        dir,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The plugin and entry id behind `plugin:<name>/<id>`, only while it is installed and on.
pub fn resolve(paths: &Paths, id: &str) -> Option<(Plugin, String)> {
    let (name, Some(eid)) = split_id(id)? else {
        return None;
    };
    if !valid_name(name) || !valid_id(eid) {
        return None;
    }
    let p = list(paths).into_iter().find(|p| p.name == name && p.on)?;
    Some((p, eid.to_string()))
}

/// Text without ANSI escapes or other control characters (status text may carry colour for bash).
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                // CSI: parameters up to a final byte @..~
                for c in it.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// `plugin rows` output: `section<TAB>title` first, then `<id><TAB><label><TAB><status>` per
/// entry, each optionally followed by `  <TAB><text>` detail lines. Anything else is ignored.
pub fn parse_rows(text: &str) -> Rows {
    let mut rows = Rows::default();
    let mut first = true;
    // after an entry line with a bad id, its detail lines belong to nothing
    let mut skip = false;
    for line in text.lines().map(|l| l.trim_end_matches('\r')) {
        if let Some(d) = line.strip_prefix("  \t") {
            if !skip && let Some(e) = rows.entries.last_mut() {
                e.details.push(plain(d));
            }
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["section", t, ..] => {
                if first {
                    rows.title = plain(t);
                }
            }
            // more than 3 fields: the rest is the status, joined with spaces
            [id, label, status @ ..] if valid_id(id) => {
                skip = false;
                rows.entries.push(PEntry {
                    id: id.to_string(),
                    label: plain(label),
                    status: plain(&status.join(" ")),
                    details: Vec::new(),
                })
            }
            [_, _, ..] => skip = true,
            _ => {}
        }
        first = false;
    }
    rows
}

/// `plugin actions` output: `<action><TAB><label>` lines.
pub fn parse_actions(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (a, label) = l.trim_end_matches('\r').split_once('\t')?;
            (valid_id(a) && !label.contains('\t')).then(|| (a.to_string(), plain(label)))
        })
        .collect()
}

/// Reads a pipe to the end on its own thread (a full pipe must never stall the plugin).
fn drain(
    r: Option<impl Read + Send + 'static>,
) -> (Arc<Mutex<Vec<u8>>>, std::thread::JoinHandle<()>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let b = buf.clone();
    let h = std::thread::spawn(move || {
        let Some(mut r) = r else { return };
        let mut chunk = [0u8; 8192];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            b.lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(&chunk[..n]);
        }
    });
    (buf, h)
}

/// What a drained pipe got. A background job the plugin left behind may hold the pipe open,
/// so this waits only briefly for the end.
fn take((buf, h): (Arc<Mutex<Vec<u8>>>, std::thread::JoinHandle<()>)) -> String {
    let end = Instant::now() + Duration::from_millis(200);
    while !h.is_finished() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    let b = buf.lock().unwrap_or_else(|e| e.into_inner());
    String::from_utf8_lossy(&b).into_owned()
}

/// Runs `<dir>/plugin <args>` without a terminal (stdin closed), in the current folder, with
/// `AI_PLUGIN_DIR`. Ok(stdout) on exit 0; Err(last stderr line or why) otherwise. Past
/// `timeout` the plugin and everything it started are killed.
pub fn run(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let exe = dir.join("plugin");
    // Windows: no exec bit there, so through Git Bash
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("bash");
        c.arg(&exe);
        c
    } else {
        Command::new(&exe)
    };
    cmd.args(args).env("AI_PLUGIN_DIR", dir);
    exec(cmd, timeout)
}

/// Runs `cmd` with stdin closed and its output piped. Ok(stdout) on exit 0; Err(last stderr
/// line or why) otherwise. Past `timeout` it and everything it started are killed.
fn exec(mut cmd: Command, timeout: Duration) -> Result<String, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // its own process group, so a timeout kills what it started too
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| format!("could not run: {e}"))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let end = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
        }
    };
    let (out, err) = (take(out), take(err));
    if status.success() {
        return Ok(out);
    }
    // some plugins print their errors on stdout
    Err(last_line(&err)
        .or_else(|| last_line(&out))
        .unwrap_or_else(|| match status.code() {
            Some(c) => format!("exit {c}"),
            None => "killed".into(),
        }))
}

/// The last non-empty line, for the flash bar.
pub fn last_line(text: &str) -> Option<String> {
    text.lines()
        .map(|l| plain(l).trim().to_string())
        .rfind(|l| !l.is_empty())
}

fn executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    p.is_file()
}

/// A git source: an `https://` URL, or GitHub `owner/repo` shorthand (https only, no ssh)
/// (each part `[A-Za-z0-9._-]+`). Same rule as bash `ai plugin add`.
fn git_url(source: &str) -> Option<String> {
    if source.starts_with("https://") {
        return Some(source.to_string());
    }
    let (o, r) = source.split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    (ok(o) && ok(r)).then(|| format!("https://github.com/{o}/{r}.git"))
}

/// Plugin name for a git source: its last `/` part, without `.git` (same as bash).
fn name_of(source: &str) -> String {
    let last = source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// $HOME, or %USERPROFILE% on Windows where HOME is often unset.
fn home() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|h| !h.is_empty())
}

/// A leading `~/` means the home folder.
pub fn expand_home(s: &str) -> String {
    match (s.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => format!("{}/{rest}", h.trim_end_matches(['/', '\\'])),
        _ => s.to_string(),
    }
}

/// Text for display without any `user:token@` after a `scheme://` (also inside git's errors).
pub fn hide_userinfo(s: &str) -> String {
    let (mut out, mut rest) = (String::new(), s);
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        let end = tail
            .find(|c: char| c == '/' || c == '\'' || c == '"' || c.is_whitespace())
            .unwrap_or(tail.len());
        out.push_str(head);
        out.push_str(tail[..end].rsplit('@').next().unwrap_or(""));
        rest = &tail[end..];
    }
    out + rest
}

/// Installs a plugin from `source`: a bundled name (`cliproxy`), a local folder (copied), or a
/// git URL / `owner/repo` (`git clone --depth 1`, URL saved in `.source`). Never overwrites.
/// Returns the installed name.
pub fn add(paths: &Paths, source: &str) -> Result<String, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("Give a folder, a git URL, owner/repo, or cliproxy.".into());
    }
    // bundled name, then an existing folder, then git: the same order as bash
    let bundled = source == "cliproxy";
    let folder = Path::new(source);
    let is_dir = !bundled && folder.is_dir();
    let git = (!bundled && !is_dir)
        .then(|| git_url(source.trim_end_matches('/')))
        .flatten();
    let name = match (bundled, is_dir) {
        (true, _) => source.to_string(),
        // the folder's name as typed, not where a link leads
        (false, true) => source
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string(),
        (false, false) if git.is_some() => name_of(source),
        (false, false) if source.starts_with("git@") || source.starts_with("ssh://") => {
            return Err("use an https:// URL".into());
        }
        _ => {
            return Err(format!(
                "{source}: not a folder, git URL or bundled plugin."
            ));
        }
    };
    if !valid_name(&name) {
        return Err(format!(
            "Bad plugin name '{name}': letters, digits, _ or - only."
        ));
    }
    let dest = paths.plugins.join(&name);
    if dest.symlink_metadata().is_ok() {
        return Err(format!("{name} is already installed."));
    }
    if !bundled && git.is_none() && !executable(&folder.join("plugin")) {
        return Err(format!("{source} has no executable 'plugin'."));
    }
    std::fs::create_dir_all(&paths.plugins).map_err(|e| e.to_string())?;
    let done = match (bundled, git) {
        (true, _) => write_bundled(&dest, &CLIPROXY),
        (false, Some(url)) => clone(&url, &dest),
        _ => crate::failover::copy_dir(folder, &dest)
            .map_err(|e| e.to_string())
            .inspect(|()| {
                // a copy starts on and is not git-updatable
                let _ = std::fs::remove_file(dest.join(".off"));
                let _ = std::fs::remove_file(dest.join(".source"));
            }),
    };
    let done = done.and_then(|()| {
        if executable(&dest.join("plugin")) {
            Ok(())
        } else {
            Err(format!("{source} has no executable 'plugin'."))
        }
    });
    if let Err(e) = done {
        // a half install would look installed: take it back
        let _ = config::remove_entry(&dest);
        return Err(e);
    }
    Ok(name)
}

fn write_bundled(dest: &Path, files: &[(&str, &str, bool)]) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for (f, text, exec) in files {
        let p = dest.join(f);
        std::fs::write(&p, text).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if *exec {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }
        #[cfg(not(unix))]
        let _ = exec;
    }
    Ok(())
}

fn git(args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    // never ask for a password: the TUI owns the terminal
    cmd.args(args).env("GIT_TERMINAL_PROMPT", "0");
    exec(cmd, GIT_TIMEOUT).map_err(|e| format!("git: {}", hide_userinfo(&e)))
}

fn clone(url: &str, dest: &Path) -> Result<(), String> {
    git(&["clone", "--depth", "1", "--", url, &dest.to_string_lossy()])?;
    std::fs::write(dest.join(".source"), format!("{url}\n")).map_err(|e| e.to_string())
}

/// `git pull --ff-only` for a plugin installed from git; returns git's last line.
pub fn update(paths: &Paths, name: &str) -> Result<String, String> {
    let dir = checked(paths, name)?;
    if !from_git(&dir) {
        return Err(format!("{name} was not installed from git."));
    }
    let out = git(&["-C", &dir.to_string_lossy(), "pull", "--ff-only"])?;
    Ok(last_line(&out).unwrap_or_else(|| "up to date".into()))
}

/// Whether the plugin in `dir` came from `git clone` (so `update` can pull it).
pub fn from_git(dir: &Path) -> bool {
    dir.join(".source").is_file() && dir.join(".git").is_dir()
}

/// Where a plugin came from, for display: its `.source` URL without userinfo, `bundled` for
/// the cliproxy written from this binary, else `local`.
// shortcut: bundled is told by an unchanged `plugin` file, so one from an older release shows local
pub fn source(p: &Plugin) -> String {
    if let Ok(s) = std::fs::read_to_string(p.dir.join(".source"))
        && !s.trim().is_empty()
    {
        return hide_userinfo(s.trim());
    }
    let bundled = p.name == "cliproxy"
        && std::fs::read_to_string(p.dir.join("plugin")).is_ok_and(|t| t == CLIPROXY[0].1);
    if bundled { "bundled" } else { "local" }.to_string()
}

/// The folder of an installed plugin, after checking the name.
fn checked(paths: &Paths, name: &str) -> Result<PathBuf, String> {
    let dir = paths.plugins.join(name);
    if !valid_name(name) || dir.symlink_metadata().is_err() {
        return Err(format!("No plugin named '{name}'."));
    }
    Ok(dir)
}

/// Removes a plugin for good; a symlinked plugin loses only the link.
pub fn rm(paths: &Paths, name: &str) -> Result<(), String> {
    let dir = checked(paths, name)?;
    config::remove_entry(&dir).map_err(|e| e.to_string())
}

/// Turns a plugin on (removes `.off`) or off (writes an empty `.off`).
pub fn set_on(paths: &Paths, name: &str, on: bool) -> Result<(), String> {
    let off = checked(paths, name)?.join(".off");
    let r = if on {
        std::fs::remove_file(&off).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(e),
        })
    } else {
        std::fs::write(&off, "")
    };
    r.map_err(|e| e.to_string())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("ai-plug-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Paths {
            home: root.join("home"),
            providers: root.join("cfg/providers"),
            cla_root: root.join("acc"),
            ai_accounts: root.join("ai-acc"),
            fo_order: root.join("cfg/failover"),
            fo_state: root.join("state"),
            codex_home: root.join("home/.codex"),
            cooldown: Duration::from_secs(60),
            plugins: root.join("cfg/plugins"),
        }
    }

    /// A plugin folder at `dir` whose `plugin` is this shell script.
    fn script(dir: &Path, body: &str) {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join("plugin");
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        warm(&p);
    }

    /// Runs a new script once, untimed: macOS checks a new executable on its first run, which
    /// can take seconds while other tests start theirs, and would trip the timeouts under test.
    pub fn warm(p: &Path) {
        let _ = Command::new(p)
            .arg("warm")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[test]
    fn parse_rows_section_lines_never_become_entries() {
        let r = parse_rows("section\tT\tx\ty\nsection\tz\tw\na\tb\tc\n");
        assert_eq!(r.title, "T");
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].id, "a");
    }

    #[test]
    fn parse_rows_reads_section_entries_and_details_and_skips_junk() {
        let r = parse_rows(
            "section\tCLIPROXY · all\n\
             all\tClaude Code · all accounts\t\x1b[32mproxy up\x1b[0m\n\
             \x20\x20\tclaude  5h 40%\n\
             junk line\n\
             bad/id\tX\tY\n\
             \x20\x20\topus  3 req\n\
             two\tSecond\n\
             section\tlate title is ignored\n\
             a\tb\tc\td\n\
             bad id\tX\n\
             \x20\x20\tnot two's\n",
        );
        assert_eq!(r.title, "CLIPROXY · all");
        assert_eq!(
            r.entries,
            vec![
                PEntry {
                    id: "all".into(),
                    label: "Claude Code · all accounts".into(),
                    status: "proxy up".into(),
                    details: vec!["claude  5h 40%".into()],
                },
                PEntry {
                    id: "two".into(),
                    label: "Second".into(),
                    ..Default::default()
                },
                PEntry {
                    id: "a".into(),
                    label: "b".into(),
                    status: "c d".into(),
                    ..Default::default()
                },
            ]
        );
        // a detail line before any entry belongs to nothing
        assert!(parse_rows("  \tlost\n").entries.is_empty());
        assert_eq!(parse_rows("").title, "");
    }

    #[test]
    fn parse_actions_keeps_valid_lines() {
        assert_eq!(
            parse_actions("start\tstart proxy\nnope\n../x\tbad\nstop\tstop proxy\r\n"),
            vec![
                ("start".into(), "start proxy".into()),
                ("stop".into(), "stop proxy".into())
            ]
        );
    }

    #[test]
    fn names_and_ids_reject_paths_and_spaces() {
        for bad in ["", "../x", "a/b", "a b", ".", "..", "x.y"] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_name("cli_proxy-2"));
        for bad in ["", "a/b", "a b", "a\tb"] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert!(valid_id("all.v2_x-1"));
        assert_eq!(split_id("plugin:x/y"), Some(("x", Some("y"))));
        assert_eq!(split_id("plugin:x"), Some(("x", None)));
        assert_eq!(split_id("claude/x"), None);
    }

    #[cfg(unix)]
    #[test]
    fn run_returns_stdout_and_sets_the_plugin_dir() {
        let p = tmp("run");
        let d = p.plugins.join("good");
        script(&d, "echo \"$1 $AI_PLUGIN_DIR\"");
        assert_eq!(
            run(&d, &["rows"], ROWS_TIMEOUT).unwrap(),
            format!("rows {}\n", d.display())
        );
        script(&d, "echo oops >&2; exit 3");
        assert_eq!(run(&d, &["rows"], ROWS_TIMEOUT), Err("oops".into()));
    }

    #[cfg(unix)]
    #[test]
    fn run_error_falls_back_to_the_last_stdout_line() {
        let d = tmp("stdout").plugins;
        script(&d, "echo first; echo 'bad key'; exit 2");
        assert_eq!(
            run(&d, &["x"], Duration::from_secs(5)).unwrap_err(),
            "bad key"
        );
    }

    #[test]
    fn run_kills_a_hanging_plugin_on_time() {
        let p = tmp("hang");
        let d = p.plugins.join("slow");
        script(&d, "[ \"$1\" = warm ] || sleep 30");
        let t = Instant::now();
        assert!(run(&d, &["rows"], Duration::from_secs(1)).is_err());
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[cfg(unix)]
    #[test]
    fn list_and_resolve_follow_off_and_skip_bad_folders() {
        let p = tmp("list");
        script(&p.plugins.join("b"), "true");
        script(&p.plugins.join("a"), "true");
        fs::create_dir_all(p.plugins.join("noplugin")).unwrap();
        script(&p.plugins.join("bad name"), "true");
        let names: Vec<String> = list(&p).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(
            resolve(&p, "plugin:a/x").map(|(p, id)| (p.name, id)),
            Some(("a".into(), "x".into()))
        );
        set_on(&p, "a", false).unwrap();
        assert!(p.plugins.join("a/.off").exists());
        assert!(!list(&p)[0].on);
        assert!(resolve(&p, "plugin:a/x").is_none(), "off");
        set_on(&p, "a", true).unwrap();
        set_on(&p, "a", true).unwrap();
        assert!(list(&p)[0].on);
        for gone in [
            "plugin:zz/x",
            "plugin:a",
            "plugin:a/b c",
            "plugin:../a/x",
            "claude/a",
        ] {
            assert!(resolve(&p, gone).is_none(), "{gone}");
        }
        assert!(set_on(&p, "../a", false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn add_copies_a_folder_or_writes_cliproxy_and_never_overwrites() {
        let p = tmp("add");
        let src = p.home.join("src/mine");
        script(&src, "true");
        fs::write(src.join(".off"), "").unwrap();
        fs::write(src.join(".source"), "https://x/y\n").unwrap();
        fs::write(src.join("data"), "x").unwrap();
        assert_eq!(add(&p, &src.to_string_lossy()), Ok("mine".into()));
        assert_eq!(
            fs::read_to_string(p.plugins.join("mine/data")).unwrap(),
            "x"
        );
        assert!(!p.plugins.join("mine/.off").exists());
        assert!(!p.plugins.join("mine/.source").exists());
        assert!(
            add(&p, &src.to_string_lossy())
                .unwrap_err()
                .contains("already")
        );

        assert_eq!(add(&p, "cliproxy"), Ok("cliproxy".into()));
        assert!(executable(&p.plugins.join("cliproxy/plugin")));
        assert!(p.plugins.join("cliproxy/config.example").is_file());
        assert!(add(&p, "cliproxy").is_err());

        let empty = p.home.join("src/empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(add(&p, &empty.to_string_lossy()).is_err());
        assert!(!p.plugins.join("empty").exists());
        assert!(add(&p, "no such thing").is_err());
        assert!(add(&p, "").is_err());

        // the name is the folder as typed (trailing / trimmed), not where a link leads
        let real = p.home.join("src/real");
        script(&real, "true");
        std::os::unix::fs::symlink(&real, p.home.join("src/linked")).unwrap();
        let typed = format!("{}/", p.home.join("src/linked").display());
        assert_eq!(add(&p, &typed), Ok("linked".into()));
        // `.` and `..` are never a name
        let dot = format!("{}/.", src.display());
        assert!(add(&p, &dot).unwrap_err().contains("Bad plugin name"));
    }

    #[test]
    fn git_sources_and_names() {
        assert_eq!(
            git_url("o/r").as_deref(),
            Some("https://github.com/o/r.git")
        );
        assert_eq!(
            git_url("my.org/r_2-x").as_deref(),
            Some("https://github.com/my.org/r_2-x.git")
        );
        for url in ["https://h/o/r"] {
            assert_eq!(git_url(url).as_deref(), Some(url));
        }
        for bad in [
            "a/b/c",
            "/x",
            "o/",
            "o/r b",
            "git://h/o/r",
            "http://h/o/r",
            "file:///tmp/r",
            "ssh://h/o/r",
            "git@github.com:o/r.git",
        ] {
            assert_eq!(git_url(bad), None, "{bad}");
        }
        assert_eq!(name_of("https://github.com/o/my-plug.git"), "my-plug");
        assert_eq!(name_of("https://h/o/plug/"), "plug");
        assert_eq!(name_of("owner/repo/"), "repo");
        assert_eq!(
            hide_userinfo("https://u:tok@h/o/r.git"),
            "https://h/o/r.git"
        );
        assert_eq!(hide_userinfo("git@h:o/r"), "git@h:o/r");
        assert_eq!(hide_userinfo("o/r"), "o/r");
        assert_eq!(
            hide_userinfo(
                "fatal: could not read Password for 'https://tok@github.com': x https://a:b@h/p"
            ),
            "fatal: could not read Password for 'https://github.com': x https://h/p"
        );
        let e = git(&[
            "clone",
            "-q",
            "--",
            "https://tok@127.0.0.1:9/x.git",
            "/nonexistent-ai-tmp/x",
        ])
        .unwrap_err();
        assert!(!e.contains("tok"), "{e}");
        let h = home().unwrap();
        assert_eq!(
            expand_home("~/x/y"),
            format!("{}/x/y", h.trim_end_matches(['/', '\\']))
        );
        assert_eq!(expand_home("~x"), "~x");
    }

    #[cfg(unix)]
    #[test]
    fn add_refuses_a_git_source_whose_name_is_bad() {
        let p = tmp("gitname");
        let e = add(&p, "https://h/o/bad name.git").unwrap_err();
        assert!(e.contains("Bad plugin name"), "{e}");
        for s in ["git@host:o/r.git", "ssh://h/o/r"] {
            assert_eq!(add(&p, s).unwrap_err(), "use an https:// URL");
        }
        assert!(add(&p, "git://h/o/r").unwrap_err().contains("not a folder"));
    }

    #[cfg(unix)]
    #[test]
    fn add_from_git_saves_the_source_and_update_pulls() {
        let p = tmp("git");
        let repo = p.home.join("repo/gplug");
        script(&repo, "true");
        let g = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(["-C", &repo.to_string_lossy()])
                    .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            )
        };
        g(&["init", "-q"]);
        g(&["add", "."]);
        g(&["commit", "-qm", "one"]);
        // file:// is not an `add` source; clone is what add runs for a git one
        let url = format!("file://{}", repo.display());
        fs::create_dir_all(&p.plugins).unwrap();
        clone(&url, &p.plugins.join("gplug")).unwrap();
        assert_eq!(
            fs::read_to_string(p.plugins.join("gplug/.source")).unwrap(),
            format!("{url}\n")
        );
        fs::write(repo.join("new"), "n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "two"]);
        assert!(update(&p, "gplug").is_ok());
        assert!(p.plugins.join("gplug/new").is_file());
        assert!(update(&p, "nope").is_err());
        // a .source without a git checkout is not updatable
        fs::remove_dir_all(p.plugins.join("gplug/.git")).unwrap();
        assert!(
            update(&p, "gplug")
                .unwrap_err()
                .contains("not installed from git")
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_never_waits_on_a_prompt() {
        // an https URL to nowhere: no prompt, stdin is closed
        let t = Instant::now();
        assert!(git(&["ls-remote", "https://127.0.0.1:1/x"]).is_err());
        assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
    }

    #[cfg(unix)]
    #[test]
    fn rm_removes_only_the_link_of_a_symlinked_plugin() {
        let p = tmp("rm");
        let real = p.home.join("real");
        script(&real, "true");
        fs::create_dir_all(&p.plugins).unwrap();
        std::os::unix::fs::symlink(&real, p.plugins.join("linked")).unwrap();
        rm(&p, "linked").unwrap();
        assert!(p.plugins.join("linked").symlink_metadata().is_err());
        assert!(real.join("plugin").is_file());
        script(&p.plugins.join("own"), "true");
        rm(&p, "own").unwrap();
        assert!(!p.plugins.join("own").exists());
        assert!(rm(&p, "..").is_err());
        assert!(rm(&p, "own").is_err());
    }
}
