use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use super::{TextInput, workspace_panel};

pub(crate) struct HerdrPrompt {
    pub(crate) input: TextInput,
    pub(crate) error: Option<String>,
    pub(crate) sending: bool,
    sender: Sender<Result<String, String>>,
    receiver: Receiver<Result<String, String>>,
}

impl Default for HerdrPrompt {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            input: TextInput::default(),
            error: None,
            sending: false,
            sender,
            receiver,
        }
    }
}

impl HerdrPrompt {
    pub(crate) fn open(&mut self) {
        self.input.clear();
        self.input.focus();
        self.error = None;
    }

    pub(crate) fn submit(&mut self) {
        if self.sending {
            return;
        }
        if self.input.text().trim().is_empty() {
            self.error = Some("Enter a command or prompt".to_owned());
            return;
        }

        let command = self.input.text().to_owned();
        let sender = self.sender.clone();
        self.error = None;
        self.sending = true;
        thread::spawn(move || {
            let _ = sender.send(workspace_panel::send_command_below(command));
        });
    }

    pub(crate) fn poll(&mut self) -> Option<Result<String, String>> {
        let result = self.receiver.try_recv().ok()?;
        self.sending = false;
        if result.is_ok() {
            self.input.clear();
        }
        Some(result)
    }
}
