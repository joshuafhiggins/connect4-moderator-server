use rand::Rng;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::vec;
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

#[derive(Clone)]
pub struct Tournament {
	pub players: HashMap<u32, SocketAddr>,
	pub top_half: Vec<u32>,
	pub bottom_half: Vec<u32>,
	pub is_completed: bool,
}

impl Tournament {
	pub fn new(ready_players: &[SocketAddr]) -> Tournament {
		let mut result = Tournament {
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

	pub fn next(&mut self) {
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
	}
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
