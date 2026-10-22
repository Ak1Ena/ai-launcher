//! Weighted round-robin for Auto launches: smooth WRR over the members that are on, weighted and
//! free, its state kept in `<fo_state>/wrr`, and an optional model asked to pick instead.

use crate::config::Paths;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

/// One smooth WRR step: every candidate gains its weight, the highest (first on a tie) or
/// `choose` is picked and pays back the total. Returns the pick and the new state.
pub fn next(
    cands: &[(String, u32)],
    state: &[(String, i64)],
    choose: Option<&str>,
) -> Option<(String, Vec<(String, i64)>)> {
    let cands: Vec<&(String, u32)> = cands.iter().filter(|(_, w)| *w > 0).collect();
    let total: i64 = cands.iter().map(|(_, w)| *w as i64).sum();
    let mut s = state.to_vec();
    for (m, w) in &cands {
        match s.iter_mut().find(|(k, _)| k == m) {
            Some(e) => e.1 += *w as i64,
            None => s.push((m.clone(), *w as i64)),
        }
    }
    let cur = |m: &str| s.iter().find(|(k, _)| k == m).map_or(0, |e| e.1);
    let pick = match choose {
        Some(c) => cands.iter().find(|(m, _)| m == c)?.0.clone(),
        None => cands
            .iter()
            .map(|(m, _)| m)
            .reduce(|a, b| if cur(b) > cur(a) { b } else { a })?
            .clone(),
    };
    if let Some(e) = s.iter_mut().find(|(k, _)| *k == pick) {
        e.1 -= total;
    }
    Some((pick, s))
}

pub fn parse_state(text: &str) -> Vec<(String, i64)> {
    text.lines()
        .filter_map(|l| {
            let (m, v) = l.split_once('\t')?;
            Some((m.to_string(), v.trim().parse().ok()?))
        })
        .collect()
}

pub fn load(paths: &Paths) -> Vec<(String, i64)> {
    parse_state(&std::fs::read_to_string(paths.fo_state.join("wrr")).unwrap_or_default())
}

pub fn save(paths: &Paths, state: &[(String, i64)]) {
    let text: String = state.iter().map(|(m, v)| format!("{m}\t{v}\n")).collect();
    let _ = std::fs::create_dir_all(&paths.fo_state);
    let _ = std::fs::write(paths.fo_state.join("wrr"), text);
}

/// Asks the model to pick. `cands`: member, weight, usage text.
pub fn prompt(cands: &[(String, u32, String)], default: &str) -> String {
    let list: String = cands
        .iter()
        .map(|(m, w, u)| format!("- {m}  weight {w}  usage {u}\n"))
        .collect();
    format!(
        "Pick the account for a new coding session. Weighted round-robin picks {default}.\n\
         Accounts (higher weight = use more often; usage = share of the plan limit used):\n{list}\
         Prefer accounts with more usage left, otherwise follow the weights.\n\
         Reply with exactly one account name from the list and nothing else."
    )
}

/// The model's reply if it is exactly one candidate name, otherwise the round-robin pick.
pub fn choose(reply: Option<&str>, cands: &[(String, u32)], default: &str) -> String {
    reply
        .map(|r| {
            r.trim()
                .trim_matches(|c| matches!(c, '`' | '"' | '\'' | '.'))
        })
        .filter(|r| cands.iter().any(|(m, _)| m == r))
        .unwrap_or(default)
        .to_string()
}

/// Runs `claude -p --model <model> <prompt>` off the UI thread; sends its reply, or None on an
/// error or after the timeout.
pub fn spawn_decide(model: String, config_dir: String, prompt: String) -> Receiver<Option<String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(ask(&model, &config_dir, &prompt));
    });
    rx
}

fn ask(model: &str, config_dir: &str, prompt: &str) -> Option<String> {
    let mut child = Command::new("claude")
        .args(["-p", "--model", model, prompt])
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let until = Instant::now() + TIMEOUT;
    while child.try_wait().ok()?.is_none() {
        if Instant::now() > until {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(list: &[(&str, u32)]) -> Vec<(String, u32)> {
        list.iter().map(|(m, w)| (m.to_string(), *w)).collect()
    }

    fn picks(cands: &[(String, u32)], n: usize) -> String {
        let mut state = Vec::new();
        (0..n)
            .map(|_| {
                let (p, s) = next(cands, &state, None).unwrap();
                state = s;
                p
            })
            .collect()
    }

    #[test]
    fn smooth_sequence_follows_weights() {
        assert_eq!(picks(&c(&[("A", 3), ("B", 1)]), 8), "AABAAABA");
        assert_eq!(picks(&c(&[("A", 1), ("B", 1), ("C", 1)]), 6), "ABCABC");
        assert_eq!(
            picks(&c(&[("A", 0), ("B", 2)]), 3),
            "BBB",
            "weight 0 is never picked"
        );
        assert!(next(&c(&[("A", 0)]), &[], None).is_none());
        assert!(next(&[], &[], None).is_none());
    }

    #[test]
    fn chosen_member_pays_and_state_round_trips() {
        let cands = c(&[("A", 3), ("B", 1)]);
        let (p, s) = next(&cands, &[], Some("B")).unwrap();
        assert_eq!(p, "B");
        assert_eq!(s, vec![("A".into(), 3), ("B".into(), -3)]);
        assert!(next(&cands, &[], Some("Z")).is_none());
        let text: String = s.iter().map(|(m, v)| format!("{m}\t{v}\n")).collect();
        assert_eq!(parse_state(&format!("{text}junk\nx\tnan\n")), s);
        // a member left out keeps its state and doesn't gain
        let (_, s2) = next(&c(&[("A", 3)]), &s, None).unwrap();
        assert_eq!(s2, vec![("A".into(), 3), ("B".into(), -3)]);
    }

    #[test]
    fn reply_must_be_one_candidate() {
        let cands = c(&[("claude/a", 1), ("cx", 1)]);
        let pick = |r: Option<&str>| choose(r, &cands, "claude/a");
        assert_eq!(pick(Some("cx\n")), "cx");
        assert_eq!(pick(Some("`cx`.")), "cx");
        assert_eq!(pick(Some("cx or claude/a")), "claude/a");
        assert_eq!(pick(Some("other")), "claude/a");
        assert_eq!(pick(Some("")), "claude/a");
        assert_eq!(pick(None), "claude/a");
        assert!(prompt(&[("cx".into(), 2, "5h 40%".into())], "cx").contains("- cx  weight 2"));
    }
}
