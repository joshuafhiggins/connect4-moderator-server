use rand::Rng;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::vec;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use crate::{broadcast_message_all_observers, gen_match_id, send, terminate_match};

pub type Clients = Arc<RwLock<HashMap<SocketAddr, Arc<RwLock<Client>>>>>;
pub type Usernames = Arc<RwLock<HashMap<String, SocketAddr>>>;
pub type Observers = Arc<RwLock<HashMap<SocketAddr, UnboundedSender<Message>>>>;
pub type Matches = Arc<RwLock<HashMap<u32, Arc<RwLock<Match>>>>>;
pub type WrappedTournament = Arc<RwLock<Option<Arc<RwLock<dyn Tournament + Send + Sync>>>>>;

pub struct Server {
	pub clients: Clients,
	pub usernames: Usernames,
	pub observers: Observers,
	pub matches: Matches,
	pub admin: Arc<RwLock<Option<SocketAddr>>>,
	pub admin_password: Arc<String>,
	pub tournament: WrappedTournament,
	pub waiting_timeout: Arc<RwLock<u64>>,
	pub demo_mode: bool,
	pub tournament_type: String,
}

#[derive(PartialEq, Clone)]
pub enum Color {
    Red,
	Yellow,
    None,
}

#[derive(Clone)]
pub struct Client {
    pub username: String,
    pub connection: UnboundedSender<Message>,
    pub ready: bool,
    pub color: Color,
    pub current_match: Option<u32>,
	pub round_robin_id: u32,
    pub score: u32,
    pub addr: SocketAddr,
}

impl Client {
    pub fn new(username: String, connection: UnboundedSender<Message>, addr: SocketAddr) -> Client {
        Client {
            username,
            connection,
            ready: false,
            color: Color::None,
            current_match: None,
			round_robin_id: 0,
            score: 0,
            addr,
        }
    }
}

#[async_trait]
pub trait Tournament {
	fn new(ready_players: &[SocketAddr]) -> Self where Self: Sized;
	async fn next(&mut self, clients: &Clients, matches: &Matches, observers: &Observers);
	async fn start(&mut self, clients: &Clients, matches: &Matches);
	async fn cancel(&mut self, clients: &Clients, matches: &Matches, observers: &Observers);
	fn is_completed(&self) -> bool;
}

#[derive(Clone)]
pub struct RoundRobin {
	pub players: HashMap<u32, SocketAddr>,
	pub top_half: Vec<u32>,
	pub bottom_half: Vec<u32>,
	pub is_completed: bool,
}

impl RoundRobin {
	async fn create_matches(&self, clients: &Clients, matches: &Matches) {
		let clients_guard = clients.read().await;
		for (i, id) in self.top_half.iter().enumerate() {
			let player1_addr = self.players.get(id).unwrap();
			let player2_addr = self.players.get(self.bottom_half.get(i).unwrap());

			if player2_addr.is_none() { continue; }
			let player2_addr = player2_addr.unwrap();

			let match_id: u32 = gen_match_id(matches).await;
			let new_match = Arc::new(RwLock::new(Match::new(
				match_id,
				*player1_addr,
				*player2_addr,
			)));

			let match_guard = new_match.read().await;
			let mut player1 = clients_guard.get(player1_addr).unwrap().write().await;

			player1.current_match = Some(match_id);
			player1.ready = false;

			if match_guard.player1 == *player1_addr {
				player1.color = Color::Red;
				let _ = send(&player1.connection, "GAME:START:1");
			} else {
				player1.color = Color::Yellow;
				let _ = send(&player1.connection, "GAME:START:0");
			}

			drop(player1);

			let mut player2 = clients_guard.get(player2_addr).unwrap().write().await;

			player2.current_match = Some(match_id);
			player2.ready = false;

			if match_guard.player1 == *player2_addr {
				player2.color = Color::Red;
				let _ = send(&player2.connection, "GAME:START:1");
			} else {
				player2.color = Color::Yellow;
				let _ = send(&player2.connection, "GAME:START:0");
			}

			drop(player2);

			matches.write().await.insert(match_id, new_match.clone());
		}
	}
}

#[async_trait]
impl Tournament for RoundRobin {
	fn new(ready_players: &[SocketAddr]) -> RoundRobin {
		let mut result = RoundRobin {
			players: HashMap::new(),
			top_half: Vec::new(),
			bottom_half: Vec::new(),
			is_completed: false,
		};

		let size = ready_players.len();

		for (id, player) in ready_players.iter().enumerate() {
			result.players.insert(id as u32, *player);
		}

		for i in 0..size / 2 {
			result.top_half.push(i as u32);
		}

		for i in size / 2..size {
			result.bottom_half.push(i as u32);
		}

		result
	}

	async fn next(&mut self, clients: &Clients, matches: &Matches, observers: &Observers) {
		if self.is_completed {
			return;
		}

		if self.top_half.len() <= 1 || self.bottom_half.is_empty() {
			self.is_completed = true;
			return;
		}

		let last_from_top = self.top_half.pop().unwrap();
		let first_from_bottom = self.bottom_half.remove(0);

		self.top_half.insert(1, first_from_bottom);
		self.bottom_half.push(last_from_top);

		let expected_bottom_start = self.top_half.len() as u32;
		if self.top_half[1] == 1 && self.bottom_half[0] == expected_bottom_start {
			self.is_completed = true;
		}

		if self.is_completed() {
			// Send scores
			let clients_guard = clients.read().await;
			let mut player_scores: Vec<(String, u32)> = Vec::new();
			for (_, player_addr) in self.players.iter() {
				let mut player = clients_guard.get(player_addr).unwrap().write().await;
				let _ = send(&player.connection.clone(), "TOURNAMENT:END");
				player_scores.push((player.username.clone(), player.score));
				player.score = 0;
				player.round_robin_id = 0;
			}

			player_scores.sort_by(|a, b| b.1.cmp(&a.1));

			let mut message = "TOURNAMENT:END:".to_string();
			for (player, score) in player_scores.iter() {
				message.push_str(&format!("{},{}|", player, score))
			}
			message.pop();

			broadcast_message_all_observers(observers, &message).await;
		}
		else {
			// Create next matches
			self.create_matches(clients, matches).await;
		}
	}

	async fn start(&mut self, clients: &Clients, matches: &Matches) {
		self.create_matches(clients, matches).await;
	}

	async fn cancel(&mut self, clients: &Clients, matches: &Matches, observers: &Observers) {
		for (_, addr) in self.players.iter() {
			let clients_guard = clients.read().await;

			let client = clients_guard.get(addr);
			if client.is_none() { continue; }
			let client = client.unwrap().read().await;
			let client_connection = client.connection.clone();
			let client_ready = client.ready;

			let match_id = client.current_match;
			if match_id.is_none() { continue; }
			let match_id = match_id.unwrap();

			drop(client);
			drop(clients_guard);

			terminate_match(match_id, matches, clients, observers, false).await;

			if !client_ready {
				let _ = send(&client_connection, "TOURNAMENT:END");
			}
		}
	}

	fn is_completed(&self) -> bool { self.is_completed }
}

pub struct Match {
    pub id: u32,
    pub board: Vec<Vec<Color>>,
    pub viewers: Vec<SocketAddr>,
    pub ledger: Vec<(Color, usize)>,
	pub move_to_dispatch: (Color, usize),
	pub wait_thread: Option<tokio::task::JoinHandle<()>>,
    pub player1: SocketAddr,
    pub player2: SocketAddr,
}

impl Match {
    pub fn new(id: u32, player1: SocketAddr, player2: SocketAddr) -> Match {
        let first = if rand::rng().random_range(0..=1) == 0 {
            player1.to_string().parse().unwrap()
        } else {
            player2.to_string().parse().unwrap()
        };

        Match {
            id,
            board: vec![vec![Color::None; 6]; 7],
            viewers: Vec::new(),
            ledger: Vec::new(),
			move_to_dispatch: (Color::None, 0),
			wait_thread: None,
            player1: if player1 == first {player1} else {player2},
            player2: if player1 == first {player2} else {player1},
        }
    }

    pub fn place_token(&mut self, color: Color, column: usize) {
        for i in 0..6 {
            if self.board[column][i] == Color::None {
                self.board[column][i] = color;
                break;
            }
        }
    }
}
