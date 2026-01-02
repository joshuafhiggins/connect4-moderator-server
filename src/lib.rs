use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use rand::Rng;
use tokio::sync::{
    mpsc::{error::SendError, UnboundedSender},
    RwLock,
};
use tokio_tungstenite::tungstenite::Message;
use tracing::error;

use crate::{
    tournaments::Tournament,
    types::{Client, Color, Match},
};

pub mod server;
pub mod tournaments;
pub mod types;

pub type Clients = Arc<RwLock<HashMap<SocketAddr, Arc<RwLock<Client>>>>>;
pub type Usernames = Arc<RwLock<HashMap<String, SocketAddr>>>;
pub type Observers = Arc<RwLock<HashMap<SocketAddr, UnboundedSender<Message>>>>;
pub type Matches = Arc<RwLock<HashMap<u32, Arc<RwLock<Match>>>>>;
pub type WrappedTournament = Arc<RwLock<Option<Arc<RwLock<dyn Tournament + Send + Sync>>>>>;

pub const SERVER_PLAYER_USERNAME: &str = "The Server";
pub const SERVER_PLAYER_ADDR: &str = "127.0.0.1:6666";

pub async fn broadcast_message(observers: &Observers, addrs: &Vec<SocketAddr>, msg: &str) {
    for addr in addrs {
        let observers_guard = observers.read().await;
        let tx = observers_guard.get(addr);
        if tx.is_none() {
            continue;
        }
        let _ = send(tx.unwrap(), msg);
    }
}

pub async fn gen_match_id(matches: &Matches) -> u32 {
    let matches_guard = matches.read().await;
    let mut result = rand::rng().random_range(100000..=999999);
    while matches_guard.get(&result).is_some() {
        result = rand::rng().random_range(100000..=999999);
    }
    result
}

pub fn random_move(board: &[Vec<Color>]) -> usize {
    let mut random = rand::rng().random_range(0..7);
    while board[random][5] != Color::None {
        random = rand::rng().random_range(0..7);
    }

    random
}

pub fn send(tx: &UnboundedSender<Message>, text: &str) -> Result<(), SendError<Message>> {
    tx.send(Message::text(text))
}
