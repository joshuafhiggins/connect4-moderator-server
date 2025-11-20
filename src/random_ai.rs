use crate::types::Color;
use rand::Rng;

pub fn random_move(board: &[Vec<Color>]) -> usize {
    let mut random = rand::rng().random_range(0..7);
    while board[random][5] != Color::None {
        random = rand::rng().random_range(0..7);
    }

    random
}
