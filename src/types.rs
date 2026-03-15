use rand::RngExt;
use std::net::SocketAddr;
use std::time::Instant;
use std::{ops, vec};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;

#[derive(PartialEq, Clone, Copy)]
pub enum Color {
    Red,
    Yellow,
    None,
}

impl ops::Not for Color {
    type Output = Color;

    fn not(self) -> Color {
        match self {
            Color::Red => Color::Yellow,
            Color::Yellow => Color::Red,
            Color::None => Color::None,
        }
    }
}

impl From<Color> for bool {
    fn from(color: Color) -> bool {
        match color {
            Color::Red => true,
            Color::Yellow => false,
            Color::None => panic!("Cannot convert Color::None to bool"),
        }
    }
}

#[derive(Clone)]
pub struct Client {
    pub username: String,
    pub connection: UnboundedSender<Message>,
    pub ready: bool,
    pub color: Color,
    pub current_match: Option<u32>,
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
            addr,
        }
    }
}

pub struct Match {
    pub id: u32,
    pub demo_mode: bool,
    pub board: Vec<Vec<Color>>,
    pub ledger: Vec<(Color, usize, Instant)>,
    pub wait_thread: Option<tokio::task::JoinHandle<()>>,
    pub timeout_thread: Option<tokio::task::JoinHandle<()>>,
    pub player1: SocketAddr,
    pub player2: SocketAddr,
}

impl Match {
    pub fn new(id: u32, player1: SocketAddr, player2: SocketAddr, demo_mode: bool) -> Match {
        let first = if rand::rng().random_range(0..=1) == 0 {
            player1.to_string().parse().unwrap()
        } else {
            player2.to_string().parse().unwrap()
        };

        Match {
            id,
            demo_mode,
            board: vec![vec![Color::None; 6]; 7],
            ledger: Vec::new(),
            wait_thread: None,
            timeout_thread: None,
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

    pub fn end_game_check(&self) -> (Color, bool) {
        let mut result = (Color::None, false);

        let mut any_empty = true;
        for x in 0..7 {
            for y in 0..6 {
                let color = self.board[x][y].clone();
                let mut horizontal_end = true;
                let mut vertical_end = true;
                let mut diagonal_end_up = true;
                let mut diagonal_end_down = true;

                if any_empty && color == Color::None {
                    any_empty = false;
                }

                for i in 0..4 {
                    if x + i >= 7 || self.board[x + i][y] != color && horizontal_end {
                        horizontal_end = false;
                    }

                    if y + i >= 6 || self.board[x][y + i] != color && vertical_end {
                        vertical_end = false;
                    }

                    if x + i >= 7
                        || y + i >= 6
                        || self.board[x + i][y + i] != color && diagonal_end_up
                    {
                        diagonal_end_up = false;
                    }

                    if x + i >= 7
                        || (y as i32 - i as i32) < 0
                        || self.board[x + i][y - i] != color && diagonal_end_down
                    {
                        diagonal_end_down = false;
                    }
                }

                if horizontal_end || vertical_end || diagonal_end_up || diagonal_end_down {
                    result = (color.clone(), false);
                    break;
                }
            }
            if result.0 != Color::None {
                break;
            }
        }

        if any_empty && result.0 == Color::None {
            result.1 = true;
        }

        result
    }
}
