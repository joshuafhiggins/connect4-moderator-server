use async_trait::async_trait;

use crate::server::Server;

pub mod round_robin;
pub use round_robin::RoundRobin;

#[async_trait]
pub trait Tournament {
  fn new(ready_players: &[String]) -> Self
  where
    Self: Sized;
  async fn next(&mut self, server: &Server);
  async fn start(&mut self, server: &Server);
  async fn cancel(&mut self, server: &Server);
  fn inform_winner(&mut self, winner: String, is_tie: bool);
  fn contains_player(&self, username: String) -> bool;
  fn is_completed(&self) -> bool;
  fn get_type(&self) -> String;
}
