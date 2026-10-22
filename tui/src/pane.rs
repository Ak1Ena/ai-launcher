//! One CLI running on a pseudo terminal, drawn from a vt100 screen. The pane answers the
//! terminal queries CLIs wait on at startup (cursor position, device attributes, colors),
//! since no real terminal sits behind it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        match (i1, c, params.first().and_then(|p| p.first())) {
            (None, 'n', Some(6)) => {
                let (row, col) = screen.cursor_position();
                self.0
                    .extend(format!("\x1b[{};{}R", row + 1, col + 1).bytes());
            }
            (None, 'n', Some(5)) => self.0.extend(b"\x1b[0n"),
            (None, 'c', _) => self.0.extend(b"\x1b[?62;22c"),
            (Some(b'>'), 'c', _) => self.0.extend(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        // foreground / background color queries: answer a dark theme
        if let [which @ (b"10" | b"11"), b"?"] = params {
            let rgb = if *which == b"10" {
                "d0d0/d0d0/d0d0"
            } else {
                "1c1c/1c1c/1c1c"
            };
            self.0
                .extend(format!("\x1b]{};rgb:{rgb}\x1b\\", String::from_utf8_lossy(which)).bytes());
        }
    }
}

pub type Screen = Arc<Mutex<vt100::Parser<Replies>>>;

pub struct Pane {
    pub screen: Screen,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16),
}

impl Pane {
    /// Runs `bash -c '<command> "$@"' ai <args>`: the saved command is the user's own shell
    /// string, as in the bash launcher.
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, Option<String>)],
        rows: u16,
        cols: u16,
    ) -> anyhow::Result<Pane> {
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new("bash");
        cmd.arg("-c");
        cmd.arg(format!("{command} \"$@\""));
        cmd.arg("ai");
        cmd.args(args);
        cmd.cwd(std::env::current_dir()?);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            }
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let screen: Screen = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            2000,
            Replies::default(),
        )));
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let mut reader = pair.master.try_clone_reader()?;
        {
            let (screen, writer) = (screen.clone(), writer.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let reply = {
                        let mut p = screen.lock().unwrap_or_else(|e| e.into_inner());
                        p.process(&buf[..n]);
                        std::mem::take(&mut p.callbacks_mut().0)
                    };
                    if !reply.is_empty() {
                        let _ = writer
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .write_all(&reply);
                    }
                }
            });
        }
        Ok(Pane {
            screen,
            writer,
            master: pair.master,
            child,
            size: (rows, cols),
        })
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.size == (rows, cols) || rows == 0 || cols == 0 {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.screen
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
    }

    // shortcut: writes on the UI thread, so a huge paste to a CLI that isn't reading blocks the
    // TUI until it reads; move writes to a thread if that shows up.
    pub fn write(&self, bytes: &[u8]) {
        let _ = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(bytes);
    }

    pub fn key(&self, key: KeyEvent) {
        let app_cursor = self
            .screen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .application_cursor();
        if let Some(b) = encode_key(key, app_cursor) {
            self.write(&b);
        }
    }

    pub fn paste(&self, text: &str) {
        let wants = self
            .screen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .bracketed_paste();
        if wants {
            self.write(bracketed(text).as_bytes());
        } else {
            self.write(text.as_bytes());
        }
    }

    /// The exit code once the CLI has ended.
    pub fn exited(&mut self) -> Option<u32> {
        self.child.try_wait().ok().flatten().map(|s| s.exit_code())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        if self.exited().is_none() {
            self.kill();
        }
    }
}

/// A paste wrapped for a CLI that asked for bracketed paste. Escapes are dropped: a pasted
/// "\x1b[201~" would end the paste early and the rest would arrive as typed keys.
pub fn bracketed(text: &str) -> String {
    format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
}

/// Bytes a terminal sends for a key press (xterm encoding).
pub fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = key.modifiers;
    let alt = m.contains(KeyModifiers::ALT);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let shift = m.contains(KeyModifiers::SHIFT);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let modp = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let csi = |final_: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{final_}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[{n};{modp}~")
        } else {
            format!("\x1b[{n}~")
        }
        .into_bytes()
    };
    let mut out = match key.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                '@' | ' ' | '2' => 0,
                '[' | '3' => 27,
                '\\' | '4' => 28,
                ']' | '5' => 29,
                '^' | '6' => 30,
                '_' | '-' | '7' => 31,
                _ => return None,
            };
            vec![b]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter if alt || shift => return Some(b"\x1b\r".to_vec()),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => return Some(csi('A')),
        KeyCode::Down => return Some(csi('B')),
        KeyCode::Right => return Some(csi('C')),
        KeyCode::Left => return Some(csi('D')),
        KeyCode::Home => return Some(csi('H')),
        KeyCode::End => return Some(csi('F')),
        KeyCode::Insert => return Some(tilde(2)),
        KeyCode::Delete => return Some(tilde(3)),
        KeyCode::PageUp => return Some(tilde(5)),
        KeyCode::PageDown => return Some(tilde(6)),
        KeyCode::F(n @ 1..=4) => {
            return Some(format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes());
        }
        KeyCode::F(n @ 5..=12) => {
            return Some(tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]));
        }
        _ => return None,
    };
    if alt {
        out.insert(0, 0x1b);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, m: KeyModifiers) -> Vec<u8> {
        encode_key(KeyEvent::new(code, m), false).unwrap()
    }

    #[test]
    fn keys_encode_like_xterm() {
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL), vec![3]);
        assert_eq!(k(KeyCode::Char('x'), KeyModifiers::ALT), b"\x1bx".to_vec());
        assert_eq!(k(KeyCode::Up, KeyModifiers::NONE), b"\x1b[A".to_vec());
        assert_eq!(
            k(KeyCode::Right, KeyModifiers::CONTROL),
            b"\x1b[1;5C".to_vec()
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), true).unwrap(),
            b"\x1bOA".to_vec()
        );
        assert_eq!(k(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b\r".to_vec());
        assert_eq!(k(KeyCode::F(5), KeyModifiers::NONE), b"\x1b[15~".to_vec());
    }

    #[test]
    fn paste_cannot_end_itself_early() {
        assert_eq!(
            bracketed("a\x1b[201~rm -rf ~\n"),
            "\x1b[200~a[201~rm -rf ~\n\x1b[201~"
        );
    }

    #[test]
    fn answers_cursor_and_device_queries() {
        let mut p = vt100::Parser::new_with_callbacks(10, 20, 0, Replies::default());
        p.process(b"ab\x1b[6n\x1b[c");
        assert_eq!(p.callbacks().0, b"\x1b[1;3R\x1b[?62;22c".to_vec());
    }
}
