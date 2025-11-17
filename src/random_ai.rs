use rand::Rng;
use crate::types::{Client, Color};

pub fn random_move(board: &[Vec<Color>]) -> usize {
	let mut random = rand::rng().random_range(0..6);
	while board[random][4] != Color::None {
		random = rand::rng().random_range(0..6);
	}

	random
}