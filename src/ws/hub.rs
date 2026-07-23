use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use tungstenite::Message;

#[derive(Clone, Default)]
pub struct WsHub {
    clients: Arc<Mutex<Vec<Sender<Message>>>>,
}

impl WsHub {
    pub fn client_count(&self) -> usize {
        self.clients
            .lock()
            .map(|clients| clients.len())
            .unwrap_or_default()
    }

    pub fn broadcast(&self, text: String) {
        self.broadcast_message(Message::Text(text));
    }

    pub fn broadcast_binary(&self, bytes: Vec<u8>) {
        self.broadcast_message(Message::Binary(bytes));
    }

    fn broadcast_message(&self, message: Message) {
        let Ok(mut clients) = self.clients.lock() else {
            return;
        };
        clients.retain(|sender| sender.send(message.clone()).is_ok());
    }

    pub(super) fn register_client(&self, sender: Sender<Message>) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.push(sender);
        }
    }
}
