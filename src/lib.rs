use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use rand::Rng;
use tokio::sync::{RwLock, mpsc::{UnboundedSender, error::SendError}};
use tokio_tungstenite::tungstenite::Message;
use tracing::error;

use crate::{tournaments::Tournament, types::{Client, Color, Match}};

pub mod types;
pub mod tournaments;

pub type Clients = Arc<RwLock<HashMap<SocketAddr, Arc<RwLock<Client>>>>>;
pub type Usernames = Arc<RwLock<HashMap<String, SocketAddr>>>;
pub type Observers = Arc<RwLock<HashMap<SocketAddr, UnboundedSender<Message>>>>;
pub type Matches = Arc<RwLock<HashMap<u32, Arc<RwLock<Match>>>>>;
pub type WrappedTournament = Arc<RwLock<Option<Arc<RwLock<dyn Tournament + Send + Sync>>>>>;

pub async fn broadcast_message(addrs: &Vec<SocketAddr>, observers: &Observers, msg: &str) {
    for addr in addrs {
		let observers_guard = observers.read().await;
		let tx = observers_guard.get(addr);
		if tx.is_none() { continue; }
        let _ = send(tx.unwrap(), msg);
    }
}

pub async fn broadcast_message_all_observers(observers: &Observers, msg: &str) {
	let observers_guard = observers.read().await;
	for (_, tx) in observers_guard.iter() {
		let _ = send(tx, msg);
	}
}

pub async fn watch(matches: &Matches, new_match_id: u32, addr: SocketAddr) -> Result<(), String> {
	let matches_guard = matches.read().await;

    for match_guard in matches_guard.values() {
        let mut found = false;
		let mut a_match = match_guard.write().await;
        for i in 0..a_match.viewers.len() {
            if a_match.viewers[i] == addr {
                a_match.viewers.remove(i);
                found = true;
                break;
            }
        }

        if found {
            break;
        }
    }

	let result = matches_guard.get(&new_match_id);
	if result.is_none() {
		return Err("Match not found".to_string());
	}
    result.unwrap().write().await.viewers.push(addr);

	Ok(())
}

pub async fn auth_check(admin: &Arc<RwLock<Option<SocketAddr>>>, addr: SocketAddr) -> bool {
	if admin.read().await.is_none() || admin.read().await.unwrap() != addr {
		return false;
	}
	true
}

pub async fn gen_match_id(matches: &Matches) -> u32 {
	let matches_guard = matches.read().await;
	let mut result = rand::rng().random_range(100000..=999999);
	while matches_guard.get(&result).is_some() {
		result = rand::rng().random_range(100000..=999999);
	}
	result
}

pub async fn terminate_match(match_id: u32, matches: &Matches, clients: &Clients, observers: &Observers, demo_mode: bool) {
	let matches_guard = matches.read().await;
	let the_match = matches_guard.get(&match_id);
	if the_match.is_none() {
		error!("Tried to call terminate_match on invalid matchID: {}", match_id);
	}
	let the_match = the_match.unwrap().read().await;

	if the_match.wait_thread.is_some() {
		the_match.wait_thread.as_ref().unwrap().abort();
	}

	broadcast_message(&the_match.viewers, observers, "GAME:TERMINATED").await;

	let clients_guard = clients.read().await;
	let mut player1 = clients_guard.get(&the_match.player1).unwrap().write().await;
	let _ = send(&player1.connection, "GAME:TERMINATED");
	player1.current_match = None;
	player1.color = Color::None;
	drop(player1);

	if !demo_mode {
		let mut player2 = clients_guard.get(&the_match.player2).unwrap().write().await;
		let _ = send(&player2.connection, "GAME:TERMINATED");
		player2.current_match = None;
		player2.color = Color::None;
		drop(player2);
	}

	drop(clients_guard);

	drop(the_match);
	drop(matches_guard);

	matches.write().await.remove(&match_id);
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