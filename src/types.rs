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

	pub fn send(&self, text: String) -> Result<(), SendError<Message>> {
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
	pub id: u32,
	pub board: Vec<Vec<Color>>,
	pub player1: AI,
	pub player2: AI,
}

impl Match {
	pub fn new(id: u32, player1: AI, player2: AI) -> Match {
		Match { id, board: Vec::new(), player1, player2 }
	}
}
