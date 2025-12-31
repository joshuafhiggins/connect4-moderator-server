use std::net::SocketAddr;

use async_trait::async_trait;

use crate::{*};

pub mod round_robin;
pub use round_robin::RoundRobin;

#[async_trait]
pub trait Tournament {
	fn new(ready_players: &[SocketAddr]) -> Self where Self: Sized;
	async fn next(&mut self, clients: &Clients, matches: &Matches, observers: &Observers);
	async fn start(&mut self, clients: &Clients, matches: &Matches);
	async fn cancel(&mut self, clients: &Clients, matches: &Matches, observers: &Observers);
	fn is_completed(&self) -> bool;
}