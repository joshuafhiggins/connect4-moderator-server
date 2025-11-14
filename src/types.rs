use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;

#[derive(PartialEq)]
pub enum Role {
    Admin,
    Observer,
    Player,
}

#[derive(PartialEq)]
pub enum Color {
	Red,
	Blue,
	None,
}

pub struct Client {
    pub client_id: String,
    pub role: Role,
    pub connection: UnboundedSender<Message>,
	pub username: Option<String>,
}

impl Client {
	pub fn new(client_id: String, role: Role, connection: UnboundedSender<Message>) -> Client {
		Client {
			client_id,
			role,
			connection,
			username: None,
		}
	}

	pub fn send(&self, text: &str) -> Result<(), SendError<Message>> {
		self.connection.send(Message::text(text))
	}
}

pub struct AI {
	pub username: String,
	pub color: Color,
	pub ready: bool,
}

impl AI {
	pub fn new(username: &str, color: Color) -> AI {
		AI { username: username.to_string(), color, ready: false }
	}
}

pub struct Match {
	pub board: Vec<Vec<Color>>,
	pub viewers: Vec<SocketAddr>,
	pub player1: AI,
	pub player2: AI,
}

impl Match {
	pub fn new(player1: AI, player2: AI) -> Match {
		Match { board: Vec::new(), viewers: Vec::new(), player1, player2 }
	}
}

pub struct MatchMaker {
	pub matches: HashMap<u32, Match>,
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