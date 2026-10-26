//! Small modal forms (add / edit an account) and the delete confirmation. Keys only; ui.rs draws them.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What an account is, on the add form. The order is the order the picker cycles through.
pub const TYPES: [&str; 4] = ["Claude account", "Codex login", "Gemini login", "Other CLI"];

pub enum Input {
    Text { value: String, cursor: usize },
    Choice { options: Vec<String>, at: usize },
}

pub struct Field {
    pub label: &'static str,
    pub hint: String,
    pub input: Input,
    pub locked: bool,
}

impl Field {
    pub fn text(label: &'static str, value: &str, hint: impl Into<String>) -> Field {
        Field {
            label,
            hint: hint.into(),
            input: Input::Text {
                value: value.to_string(),
                cursor: value.chars().count(),
            },
            locked: false,
        }
    }

    pub fn choice(label: &'static str, options: &[&str], hint: impl Into<String>) -> Field {
        Field {
            label,
            hint: hint.into(),
            input: Input::Choice {
                options: options.iter().map(|o| o.to_string()).collect(),
                at: 0,
            },
            locked: false,
        }
    }

    pub fn locked(mut self) -> Field {
        self.locked = true;
        self
    }

    pub fn value(&self) -> String {
        match &self.input {
            Input::Text { value, .. } => value.trim().to_string(),
            Input::Choice { options, at } => options[*at].clone(),
        }
    }

    fn set(&mut self, v: &str) {
        if let Input::Text { value, cursor } = &mut self.input {
            *value = v.to_string();
            *cursor = v.chars().count();
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    Add,
    Edit(String),
    /// the model that picks the account for an Auto launch
    Model,
    /// install a plugin from a source
    AddPlugin,
    /// run one of plugin `<name>`'s actions
    Action(String),
}

pub struct Form {
    pub title: String,
    pub purpose: Purpose,
    pub fields: Vec<Field>,
    /// the focused field; `fields.len()` is the Save button
    pub focus: usize,
    pub error: Option<String>,
    /// label and command still follow the type and name (until the user edits them)
    auto: (bool, bool),
}

pub enum Outcome {
    Open,
    Cancel,
    Save,
}

impl Form {
    pub fn add() -> Form {
        let mut f = Form {
            title: "Add an account".into(),
            purpose: Purpose::Add,
            fields: vec![
                Field::choice("Type", &TYPES, "←  → to change"),
                Field::text("Name", "", "one word, e.g. work"),
                Field::text("Label", "", "what the list shows"),
                Field::text("Command", "", "how to start it"),
            ],
            focus: 0,
            error: None,
            auto: (true, true),
        };
        f.fill_defaults();
        f
    }

    pub fn edit(id: &str, label: &str, command: &str, shared: bool) -> Form {
        let hint = if shared {
            "shared by every Claude account"
        } else {
            "how to start it"
        };
        Form {
            title: format!("Edit {id}"),
            purpose: Purpose::Edit(id.to_string()),
            fields: vec![
                Field::text("Name", id, "can't be changed here").locked(),
                Field::text("Label", label, "what the list shows"),
                Field::text("Command", command, hint),
            ],
            focus: 1,
            error: None,
            auto: (true, true),
        }
    }

    pub fn model(current: &str) -> Form {
        Form {
            title: "Auto launch model".into(),
            purpose: Purpose::Model,
            fields: vec![Field::text(
                "Model",
                current,
                "e.g. haiku · empty = plain round-robin",
            )],
            focus: 0,
            error: None,
            auto: (true, true),
        }
    }

    pub fn add_plugin() -> Form {
        Form {
            title: "Add a plugin".into(),
            purpose: Purpose::AddPlugin,
            fields: vec![Field::text(
                "Source",
                "",
                "folder · git URL · owner/repo · cliproxy",
            )],
            focus: 0,
            error: None,
            auto: (true, true),
        }
    }

    /// A plugin's action menu: `labels` from `plugin actions`.
    pub fn actions(name: &str, labels: Vec<String>) -> Form {
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        Form {
            title: format!("{name} actions"),
            purpose: Purpose::Action(name.to_string()),
            fields: vec![Field::choice("Action", &labels, "←  → to change")],
            focus: 0,
            error: None,
            auto: (true, true),
        }
    }

    /// The chosen option's position in a choice field.
    pub fn index(&self, label: &str) -> usize {
        match self
            .fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| &f.input)
        {
            Some(Input::Choice { at, .. }) => *at,
            _ => 0,
        }
    }

    pub fn get(&self, label: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(Field::value)
            .unwrap_or_default()
    }

    /// The add form's label and command follow the type until the user types their own.
    fn fill_defaults(&mut self) {
        if self.purpose != Purpose::Add {
            return;
        }
        let (label, command) = match self.get("Type").as_str() {
            "Claude account" => ("Claude account", "claude"),
            "Codex login" => ("Codex", "codex"),
            "Gemini login" => ("Gemini", "gemini"),
            _ => ("", ""),
        };
        let name = self.get("Name");
        let auto = self.auto;
        for f in &mut self.fields {
            match f.label {
                "Label" if auto.0 => f.set(format!("{label} {name}").trim()),
                "Command" if auto.1 => f.set(command),
                _ => {}
            }
        }
        let claude = self.get("Type") == "Claude account";
        // a Claude account runs the shared `claude` command, so neither label nor command is its own
        for f in &mut self.fields {
            if matches!(f.label, "Label" | "Command") {
                f.locked = claude;
            }
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        self.error = None;
        let last = self.fields.len();
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter if self.focus == last => return Outcome::Save,
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Outcome::Save;
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down => self.step(1),
            KeyCode::BackTab | KeyCode::Up => self.step(-1),
            _ if self.focus < last => self.edit_field(key),
            _ => {}
        }
        Outcome::Open
    }

    /// Pasted text goes into the focused text field at the cursor; newlines are dropped.
    pub fn paste(&mut self, text: &str) {
        if self.focus >= self.fields.len()
            || !matches!(self.fields[self.focus].input, Input::Text { .. })
        {
            return;
        }
        for c in text.chars().filter(|c| !matches!(c, '\n' | '\r')) {
            self.edit_field(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn step(&mut self, by: isize) {
        let n = self.fields.len() as isize + 1;
        let mut i = self.focus as isize;
        loop {
            i = (i + by).rem_euclid(n);
            if i as usize == self.fields.len() || !self.fields[i as usize].locked {
                break;
            }
        }
        self.focus = i as usize;
    }

    fn edit_field(&mut self, key: KeyEvent) {
        match self.fields[self.focus].label {
            "Label" => self.auto.0 = false,
            "Command" => self.auto.1 = false,
            _ => {}
        }
        let f = &mut self.fields[self.focus];
        if f.locked {
            return;
        }
        match &mut f.input {
            Input::Choice { options, at } => match key.code {
                KeyCode::Left => *at = (*at + options.len() - 1) % options.len(),
                KeyCode::Right | KeyCode::Char(' ') => *at = (*at + 1) % options.len(),
                _ => return,
            },
            Input::Text { value, cursor } => {
                let mut chars: Vec<char> = value.chars().collect();
                match key.code {
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        chars.insert(*cursor, c);
                        *cursor += 1;
                    }
                    KeyCode::Char('u') => {
                        chars.drain(..*cursor);
                        *cursor = 0;
                    }
                    KeyCode::Backspace if *cursor > 0 => {
                        *cursor -= 1;
                        chars.remove(*cursor);
                    }
                    KeyCode::Delete if *cursor < chars.len() => {
                        chars.remove(*cursor);
                    }
                    KeyCode::Left => *cursor = cursor.saturating_sub(1),
                    KeyCode::Right => *cursor = (*cursor + 1).min(chars.len()),
                    KeyCode::Home => *cursor = 0,
                    KeyCode::End => *cursor = chars.len(),
                    _ => return,
                }
                *value = chars.into_iter().collect();
            }
        }
        self.fill_defaults();
    }
}

/// "Delete X?" with what deleting does.
pub struct Confirm {
    pub id: String,
    pub what: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: &mut Form, code: KeyCode) -> Outcome {
        f.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(f: &mut Form, s: &str) {
        for c in s.chars() {
            press(f, KeyCode::Char(c));
        }
    }

    #[test]
    fn paste_inserts_at_cursor_without_newlines() {
        let mut f = Form::add();
        press(&mut f, KeyCode::Right); // Codex login
        press(&mut f, KeyCode::Tab);
        typed(&mut f, "ab");
        press(&mut f, KeyCode::Left);
        f.paste("X\nY\r");
        assert_eq!(f.get("Name"), "aXYb");
    }

    #[test]
    fn add_form_follows_the_type() {
        let mut f = Form::add();
        assert_eq!(f.get("Command"), "claude");
        press(&mut f, KeyCode::Right); // Codex login
        assert_eq!(f.get("Command"), "codex");
        press(&mut f, KeyCode::Tab);
        typed(&mut f, "work");
        assert_eq!(f.get("Name"), "work");
        assert_eq!(f.get("Label"), "Codex work");
    }

    #[test]
    fn claude_account_skips_locked_fields_to_save() {
        let mut f = Form::add();
        press(&mut f, KeyCode::Tab); // Name
        assert_eq!(f.focus, 1);
        press(&mut f, KeyCode::Tab); // Label and Command are locked: straight to Save
        assert_eq!(f.focus, f.fields.len());
        assert!(matches!(press(&mut f, KeyCode::Enter), Outcome::Save));
    }

    #[test]
    fn text_edits_at_the_cursor() {
        let mut f = Form::edit("x", "abc", "cmd", false);
        press(&mut f, KeyCode::Left);
        press(&mut f, KeyCode::Backspace);
        typed(&mut f, "Z");
        assert_eq!(f.get("Label"), "aZc");
        assert!(matches!(press(&mut f, KeyCode::Esc), Outcome::Cancel));
    }
}
