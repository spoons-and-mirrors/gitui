use std::time::{Duration, Instant};

const BLINK_INTERVAL: Duration = Duration::from_millis(500);

pub(crate) struct TextInput {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    cursor_visible: bool,
    next_blink: Instant,
}

impl Default for TextInput {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            anchor: None,
            cursor_visible: true,
            next_blink: Instant::now() + BLINK_INTERVAL,
        }
    }
}

impl TextInput {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then_some({
            if anchor < self.cursor {
                (anchor, self.cursor)
            } else {
                (self.cursor, anchor)
            }
        })
    }

    pub(crate) fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.anchor = None;
        self.reset_blink();
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.anchor = None;
        self.reset_blink();
    }

    pub(crate) fn insert(&mut self, text: &str) {
        self.delete_selection();
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.reset_blink();
    }

    pub(crate) fn insert_char(&mut self, character: char) {
        self.delete_selection();
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.reset_blink();
    }

    pub(crate) fn backspace(&mut self) {
        if self.delete_selection() {
            self.reset_blink();
            return;
        }
        let start = previous_boundary(&self.text, self.cursor);
        self.text.drain(start..self.cursor);
        self.cursor = start;
        self.reset_blink();
    }

    pub(crate) fn delete(&mut self) {
        if self.delete_selection() {
            self.reset_blink();
            return;
        }
        let end = next_boundary(&self.text, self.cursor);
        self.text.drain(self.cursor..end);
        self.reset_blink();
    }

    pub(crate) fn delete_word(&mut self) {
        if self.delete_selection() {
            self.reset_blink();
            return;
        }

        let mut start = self.cursor;
        while start > 0 {
            let previous = previous_boundary(&self.text, start);
            if !self.text[previous..start].chars().all(char::is_whitespace) {
                break;
            }
            start = previous;
        }
        let word = start > 0 && {
            let previous = previous_boundary(&self.text, start);
            self.text[previous..start].chars().all(is_word_character)
        };
        while start > 0 {
            let previous = previous_boundary(&self.text, start);
            let character = self.text[previous..start]
                .chars()
                .next()
                .expect("character boundary");
            if character.is_whitespace() || is_word_character(character) != word {
                break;
            }
            start = previous;
        }
        self.text.drain(start..self.cursor);
        self.cursor = start;
        self.reset_blink();
    }

    pub(crate) fn move_left(&mut self) {
        if let Some((start, _)) = self.selection() {
            self.cursor = start;
        } else {
            self.cursor = previous_boundary(&self.text, self.cursor);
        }
        self.anchor = None;
        self.reset_blink();
    }

    pub(crate) fn move_right(&mut self) {
        if let Some((_, end)) = self.selection() {
            self.cursor = end;
        } else {
            self.cursor = next_boundary(&self.text, self.cursor);
        }
        self.anchor = None;
        self.reset_blink();
    }

    pub(crate) fn move_home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.anchor = None;
        self.reset_blink();
    }

    pub(crate) fn move_end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index);
        self.anchor = None;
        self.reset_blink();
    }

    pub(crate) fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
        self.reset_blink();
    }

    pub(crate) fn focus(&mut self) {
        self.reset_blink();
    }

    pub(crate) fn poll_blink(&mut self, focused: bool) -> bool {
        if !focused {
            self.cursor_visible = true;
            self.next_blink = Instant::now() + BLINK_INTERVAL;
            return false;
        }
        let now = Instant::now();
        if now < self.next_blink {
            return false;
        }
        self.cursor_visible = !self.cursor_visible;
        self.next_blink = now + BLINK_INTERVAL;
        true
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.drain(start..end);
        self.cursor = start;
        self.anchor = None;
        true
    }

    fn reset_blink(&mut self) {
        self.cursor_visible = true;
        self.next_blink = Instant::now() + BLINK_INTERVAL;
    }
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(index, _)| cursor + index)
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::TextInput;

    #[test]
    fn edits_unicode_and_replaces_selection() {
        let mut input = TextInput::default();
        input.set("one café");
        input.move_left();
        input.backspace();
        assert_eq!(input.text(), "one caé");

        input.select_all();
        input.insert("replacement");
        assert_eq!(input.text(), "replacement");
        assert_eq!(input.cursor(), input.text().len());
    }

    #[test]
    fn deletes_the_previous_word_and_whitespace() {
        let mut input = TextInput::default();
        input.set("subject with words   ");
        input.delete_word();
        assert_eq!(input.text(), "subject with ");
        input.delete_word();
        assert_eq!(input.text(), "subject ");
    }

    #[test]
    fn blinks_only_while_focused() {
        let mut input = TextInput {
            next_blink: std::time::Instant::now(),
            ..TextInput::default()
        };
        assert!(input.poll_blink(true));
        assert!(!input.cursor_visible());
        assert!(!input.poll_blink(false));
        assert!(input.cursor_visible());
    }
}
