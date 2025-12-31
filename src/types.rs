use crate::*;
use rand::Rng;
use std::net::SocketAddr;
use std::sync::Arc;
use std::vec;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

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

impl Server {
    pub fn new(admin_password: String, demo_mode: bool, tournament_type: String) -> Server {
        Server {
            clients: Arc::new(RwLock::new(HashMap::new())),
            usernames: Arc::new(RwLock::new(HashMap::new())),
            observers: Arc::new(RwLock::new(HashMap::new())),
            matches: Arc::new(RwLock::new(HashMap::new())),
            admin: Arc::new(RwLock::new(None)),
            admin_password: Arc::new(admin_password),
            tournament: Arc::new(RwLock::new(None)),
            waiting_timeout: Arc::new(RwLock::new(5000)),
            demo_mode,
            tournament_type,
        }
    }
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
            player1: if player1 == first { player1 } else { player2 },
            player2: if player1 == first { player2 } else { player1 },
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
