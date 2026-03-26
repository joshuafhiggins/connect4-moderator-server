use async_trait::async_trait;

use crate::server::Server;

pub mod round_robin;
pub use round_robin::RoundRobin;
pub mod knockout_bracket;
pub use knockout_bracket::KnockoutBracket;

#[async_trait]
pub trait Tournament {
  async fn new(ready_players: &[String], server: &Server) -> Self
  where
    Self: Sized;
  async fn next(&mut self, server: &Server);
  async fn start(&mut self, server: &Server);
  async fn cancel(&mut self, server: &Server);
  async fn inform_winner(
    &mut self,
    winner: String,
    match_id: u32,
    player1: String,
    player2: String,
  );
  fn contains_player(&self, username: String) -> bool;
  fn is_completed(&self) -> bool;
  fn get_players(&self) -> Vec<String>;
  fn get_type(&self) -> String;
}
