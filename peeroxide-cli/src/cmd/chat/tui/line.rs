//! Line-oriented (non-TTY) chat UI. Preserves the historical
//! `chat join` stdout contract documented in `docs/src/chat/user-guide.md` — one message per
//! line in the format `[HH:MM:SS] [name]: content`, system notices on stderr.

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::cmd::chat::display::{DisplayMessage, render_message_line};
use crate::cmd::chat::tui::{ChatUi, IgnoreSet, StatusState, UiInput, commands};

pub struct LineUi {
    status: Arc<StatusState>,
    ignore: IgnoreSet,
    input_rx: Mutex<mpsc::UnboundedReceiver<UiInput>>,
    _stdin_task: tokio::task::JoinHandle<()>,
}

impl LineUi {
    pub fn new(opts: super::UiOptions) -> Self {
        let status = StatusState::new(opts.channel_name);
        let ignore: IgnoreSet = Arc::new(RwLock::new(HashSet::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let stdin_task = tokio::spawn(stdin_task(tx));
        Self {
            status,
            ignore,
            input_rx: Mutex::new(rx),
            _stdin_task: stdin_task,
        }
    }
}

impl ChatUi for LineUi {
    fn render_message(&self, msg: &DisplayMessage) {
        let rendered = render_message_line(msg);
        for notice in &rendered.system_notices {
            eprintln!("{notice}");
        }
        println!("{}", rendered.message_line);
    }

    fn render_system(&self, line: &str) {
        eprintln!("{line}");
    }

    fn status(&self) -> Arc<StatusState> {
        self.status.clone()
    }

    fn ignore_set(&self) -> IgnoreSet {
        self.ignore.clone()
    }

    fn next_input(&mut self) -> BoxFuture<'_, Option<UiInput>> {
        Box::pin(async move {
            let mut rx = self.input_rx.lock().await;
            rx.recv().await
        })
    }

    fn shutdown(self: Box<Self>) -> BoxFuture<'static, ()> {
        // Nothing to clean up — stdin task drops naturally with the struct.
        Box::pin(async move {})
    }
}

/// Read stdin line-by-line, classify each line, forward `UiInput` into the
/// channel. On EOF, emit `UiInput::Eof` and exit.
async fn stdin_task(tx: mpsc::UnboundedSender<UiInput>) {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(text)) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let event = match commands::parse(trimmed) {
                    Some(cmd) => UiInput::Command(cmd),
                    None => UiInput::Message(trimmed.to_string()),
                };
                if tx.send(event).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = tx.send(UiInput::Eof);
                return;
            }
            Err(e) => {
                eprintln!("error reading stdin: {e}");
                let _ = tx.send(UiInput::Eof);
                return;
            }
        }
    }
}
