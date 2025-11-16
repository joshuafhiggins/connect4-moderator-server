use std::collections::HashMap;
use std::net::SocketAddr;
use std::vec;
use rand::Rng;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;

#[derive(PartialEq)]
pub enum Role {
    Admin,
    Observer,
    Player,
}

#[derive(PartialEq, Clone)]
pub enum Color {
	Red,
	Blue,
	None,
}

pub struct Client<'a> {
    pub client_id: String,
    pub role: Role,
    pub connection: UnboundedSender<Message>,
	pub username: Option<String>,
	pub current_match: Option<&'a Match<'static>>,
}

impl Client<'static> {
	pub fn new(client_id: String, role: Role, connection: UnboundedSender<Message>) -> Client<'static> {
		Client {
			client_id,
			role,
			connection,
			username: None,
			current_match: None,
		}
	}

	pub fn send(&self, text: &str) -> Result<(), SendError<Message>> {
		self.connection.send(Message::text(text))
	}
}

#[derive(Clone)]
pub struct AI {
	pub username: String,
	pub color: Color,
	pub ready: bool,
	pub addr: String,
}

impl AI {
	pub fn new(username: &str, color: Color, addr: String) -> AI {
		AI { username: username.to_string(), color, ready: false, addr }
	}
}

pub struct Match<'a> {
	pub id: u32,
	pub board: Vec<Vec<Color>>,
	pub viewers: Vec<SocketAddr>,
	pub ledger: Vec<(&'a AI, usize)>,
	pub first: AI,
	pub player1: AI,
	pub player2: AI,
}

impl Match<'static> {
	pub fn new(id: u32, player1: AI, player2: AI) -> Match<'static> {
		let first = if rand::rng().random_range(0..=1) == 0 { player1.clone() } else { player2.clone() };
		Match { id, board: vec![vec![Color::None; 5]; 6], viewers: Vec::new(), ledger: Vec::new(), first, player1, player2 }
	}
}

pub struct MatchMaker {
	pub matches: HashMap<u32, Match<'static>>,
	pub ready_players: Vec<AI>,
}

impl MatchMaker {
	pub fn new() -> MatchMaker {
		MatchMaker { matches: HashMap::new(), ready_players: Vec::new() }
	}

	pub fn watch(&mut self, viewer: SocketAddr, match_id: u32) {
		for aMatch in &mut self.matches.values_mut() {
			let mut found = false;
			for i in 0..aMatch.viewers.len() {
				if aMatch.viewers[i] == viewer {
					aMatch.viewers.remove(i);
					found = true;
					break;
				}
			}

			if found { break; }
		}

		self.matches.get_mut(&match_id).unwrap().viewers.push(viewer);
	}
}