use std::collections::HashMap;
use std::net::SocketAddr;
use std::vec;
use rand::Rng;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;

#[derive(PartialEq, Clone)]
pub enum Color {
	Red,
	Blue,
	None,
}

pub struct Client<'a> {
	pub username: String,
    pub connection: UnboundedSender<Message>,
	pub ready: bool,
	pub color: Color,
	pub current_match: Option<&'a Match<'static>>,
}

impl Client<'static> {
	pub fn new(username: String, connection: UnboundedSender<Message>) -> Client<'static> {
		Client {
			username,
			connection,
			ready: false,
			color: Color::None,
			current_match: None,
		}
	}

	pub fn send(&self, text: &str) -> Result<(), SendError<Message>> {
		self.connection.send(Message::text(text))
	}
}

pub struct Match<'a> {
	pub id: u32,
	pub board: Vec<Vec<Color>>,
	pub viewers: Vec<SocketAddr>,
	pub ledger: Vec<(&'a Client<'a>, usize)>,
	pub first: &'a Client<'a>,
	pub player1: &'a Client<'a>,
	pub player2: &'a Client<'a>,
}

impl<'a> Match<'a> {
	pub fn new(id: u32, player1: &'a Client, player2: &'a Client) -> Match<'a> {
		let first = if rand::rng().random_range(0..=1) == 0 { player1 } else { player2 };
		Match { id, board: vec![vec![Color::None; 5]; 6], viewers: Vec::new(), ledger: Vec::new(), first, player1, player2 }
	}
}