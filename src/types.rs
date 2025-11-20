use rand::Rng;
use std::net::SocketAddr;
use std::vec;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;

#[derive(PartialEq, Clone)]
pub enum Color {
    Red,
    Blue,
    None,
}

#[derive(Clone)]
pub struct Client {
    pub username: String,
    pub connection: UnboundedSender<Message>,
    pub ready: bool,
    pub color: Color,
    pub current_match: Option<u32>,
    pub demo: bool,
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
            demo: false,
            addr,
        }
    }

    pub fn send(&self, text: &str) -> Result<(), SendError<Message>> {
        self.connection.send(Message::text(text))
    }
}

pub struct Match {
    pub id: u32,
    pub board: Vec<Vec<Color>>,
    pub viewers: Vec<SocketAddr>,
    pub ledger: Vec<(Color, usize)>,
    pub first: SocketAddr,
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
		// TODO: make player1 in Match always first
        Match {
            id,
            board: vec![vec![Color::None; 6]; 7],
            viewers: Vec::new(),
            ledger: Vec::new(),
            first,
            player1,
            player2,
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
