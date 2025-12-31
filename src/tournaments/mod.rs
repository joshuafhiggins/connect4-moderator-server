use std::net::SocketAddr;

use async_trait::async_trait;

use crate::server::Server;

pub mod round_robin;
pub use round_robin::RoundRobin;

#[async_trait]
pub trait Tournament {
    fn new(ready_players: &[SocketAddr]) -> Self
    where
        Self: Sized;
    async fn next(&mut self, server: &Server);
    async fn start(&mut self, server: &Server);
    async fn cancel(&mut self, server: &Server);
    fn is_completed(&self) -> bool;
}
