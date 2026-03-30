use async_trait::async_trait;

use crate::{
  server::*,
  tournaments::{RoundRobin, Tournament},
  *,
};

type Score = u32;

#[derive(Clone)]
pub struct KnockoutBracket {
  pub blitz_round_robin: RoundRobin,
  pub players: Vec<(String, Score, bool)>,
  pub pairings: Vec<String>,
  pub current_matches: Vec<u32>,
  pub previous_wait: u64,
  pub completed: bool,
  pub started: bool,
  pub clients: Clients,
  pub matches: Matches,
  pub observers: Observers,
  pub usernames: Vec<String>,
  pub data: Vec<Vec<String>>,
}

impl KnockoutBracket {
  async fn create_matches(&mut self) {
    let clients_guard = self.clients.read().await;

    let mut i = 0;
    while i < self.pairings.len() {
      let player1_username = self.pairings[i].clone();
      let player2_username = self.pairings[i + 1].clone();

      let match_id: u32 = gen_match_id(&self.matches).await;
      self.current_matches.push(match_id);
      let new_match = Arc::new(RwLock::new(Match::new(
        match_id,
        player1_username.clone(),
        player2_username.clone(),
        false,
      )));
      let match_guard = new_match.read().await;

      let mut player1 = clients_guard.get(&player1_username).unwrap().write().await;
      player1.current_match = Some(match_id);
      player1.ready = false;
      broadcast_message(
        &self.observers,
        &format!("READY:{}:{}", player1.username.clone(), false),
      )
      .await;

      if match_guard.player1 == player1_username {
        player1.color = Color::Red;
        let _ = send(&player1.connection, "GAME:START:1");
      } else {
        player1.color = Color::Yellow;
        let _ = send(&player1.connection, "GAME:START:0");
      }
      drop(player1);

      let mut player2 = clients_guard.get(&player2_username).unwrap().write().await;
      player2.current_match = Some(match_id);
      player2.ready = false;
      broadcast_message(
        &self.observers,
        &format!("READY:{}:{}", player2.username.clone(), false),
      )
      .await;

      if match_guard.player1 == player2_username {
        player2.color = Color::Red;
        let _ = send(&player2.connection, "GAME:START:1");
      } else {
        player2.color = Color::Yellow;
        let _ = send(&player2.connection, "GAME:START:0");
      }
      drop(player2);

      self.matches.write().await.insert(match_id, new_match.clone());
      broadcast_message(
        &self.observers,
        &format!(
          "GAME:START:{},{},{}",
          match_id, match_guard.player1, match_guard.player2
        ),
      )
      .await;

      i += 2
    }
  }
}

#[async_trait]
impl Tournament for KnockoutBracket {
  async fn new(ready_players: &[String], server: &Server) -> KnockoutBracket {
    let previous_wait = server.waiting_timeout.read().await.clone();

    *server.waiting_timeout.write().await = 5;

    KnockoutBracket {
      blitz_round_robin: RoundRobin::new(ready_players, server).await,
      players: Vec::new(),
      pairings: Vec::new(),
      current_matches: Vec::new(),
      previous_wait,
      completed: false,
      started: false,
      clients: server.clients.clone(),
      matches: server.matches.clone(),
      observers: server.observers.clone(),
      usernames: ready_players.to_vec(),
      data: Vec::new(),
    }
  }

  async fn next(&mut self, server: &Server) {
    if self.completed {
      return;
    }

    if !self.started {
      self.blitz_round_robin.next(server).await;
    }

    if self.blitz_round_robin.completed && !self.started {
      self.started = true;
      *server.waiting_timeout.write().await = self.previous_wait;

      let mut players = Vec::new();
      for player in self.blitz_round_robin.players.values() {
        players.push((player.0.clone(), player.1, false));
      }

      players.sort_by(|a, b| a.1.cmp(&b.1));
      self.players = players;

      for player in &self.players {
        self.pairings.push(player.0.clone());
      }

      self.data.push(self.pairings.clone());
      self.create_matches().await;
      broadcast_message(&self.observers, &format!("GET:TOURNAMENT_DATA:{}", self.get_data().unwrap())).await;
      return;
    }

    if self.started {
      self.pairings.retain(|p| !p.is_empty());
      if self.pairings.len() == 1 {
        self.completed = true;
      } else {
        self.data.push(self.pairings.clone());
        self.create_matches().await;
        broadcast_message(&self.observers, &format!("GET:TOURNAMENT_DATA:{}", self.get_data().unwrap())).await;
      }
    }  
  }

  async fn start(&mut self, server: &Server) {
    self.blitz_round_robin.start(server).await;
  }

  async fn cancel(&mut self, server: &Server) {
    if !self.started {
      self.blitz_round_robin.cancel(server).await;
      return;
    }

    for match_id in &self.current_matches {
      server.terminate_match(*match_id).await;
    }

    let clients_guard = server.clients.read().await;
    for username in &self.players {
      let client = clients_guard.get(&username.0).cloned();
      if client.is_none() {
        continue;
      }

      let client = client.unwrap();
      let client = client.read().await;

      let _ = send(&client.connection, "TOURNAMENT:END");
    }
  }

  async fn inform_winner(
    &mut self,
    winner: String,
    match_id: u32,
    player1: String,
    player2: String,
  ) {
    if !self.started {
      self.blitz_round_robin.inform_winner(winner, match_id, player1, player2).await;
      return;
    }

    let mut winner = winner;

    // there's a tie
    if winner.is_empty() {
      let mut player1_track = (String::new(), 0, false);
      let mut player2_track = (String::new(), 0, false);

      for player in self.players.iter_mut() {
        if player.0 == player1 {
          player1_track = player.clone();
        } else if player.0 == player2 {
          player2_track = player.clone();
        }

        if !player1_track.0.is_empty() && !player2_track.0.is_empty() {
          break;
        }
      }

      if player1_track.2 {
        if player1_track.1 < player2_track.1 {
          winner = player2_track.0.clone();
        } else {
          winner = player1_track.0.clone();
        }
      }

      let match_id: u32 = gen_match_id(&self.matches).await;
      self.current_matches.push(match_id);
      let new_match = Arc::new(RwLock::new(Match::new(
        match_id,
        player1.clone(),
        player2.clone(),
        false,
      )));

      let match_guard = new_match.read().await;
      let clients_guard = self.clients.read().await;
      let mut player1 = clients_guard.get(&player1).unwrap().write().await;

      player1.current_match = Some(match_id);
      player1.ready = false;
      let player1_name = player1.username.clone();

      if match_guard.player1 == player1.username {
        player1.color = Color::Red;
        let _ = send(&player1.connection, "GAME:START:1");
      } else {
        player1.color = Color::Yellow;
        let _ = send(&player1.connection, "GAME:START:0");
      }

      drop(player1);

      let mut player2 = clients_guard.get(&player2).unwrap().write().await;

      player2.current_match = Some(match_id);
      player2.ready = false;
      let player2_name = player2.username.clone();

      if match_guard.player1 == player2.username {
        player2.color = Color::Red;
        let _ = send(&player2.connection, "GAME:START:1");
      } else {
        player2.color = Color::Yellow;
        let _ = send(&player2.connection, "GAME:START:0");
      }

      drop(player2);

      broadcast_message(
        &self.observers,
        &format!("READY:{}:{}", player1_name, false),
      )
      .await;
      broadcast_message(
        &self.observers,
        &format!("READY:{}:{}", player2_name, false),
      )
      .await;

      self.current_matches.push(match_id);
      self.matches.write().await.insert(match_id, new_match.clone());
      broadcast_message(
        &self.observers,
        &format!(
          "GAME:START:{},{},{}",
          match_id, match_guard.player1, match_guard.player2
        ),
      )
      .await;
    }

    let mut loser = String::new();
    for i in 0..self.pairings.len() {
      if self.pairings[i] == winner {
        if i % 2 == 0 {
          loser = self.pairings[i + 1].clone();
          self.pairings[i + 1].clear();
        } else {
          loser = self.pairings[i - 1].clone();
          self.pairings[i - 1].clear();
        }

        break;
      }
    }

    // Reset tie tracking
    for player in self.players.iter_mut() {
      if player.0 == winner || player.0 == loser {
        player.2 = false;
      }
    }

    self.current_matches.retain(|v| *v != match_id);
  }

  fn contains_player(&self, username: String) -> bool {
    self.usernames.contains(&username)
  }

  fn is_completed(&self) -> bool {
    self.completed
  }

  fn get_players(&self) -> Vec<String> {
    self.usernames.clone()
  }
  
  fn get_winner(&self) -> Option<String> {
    if self.completed {
      return Some(self.pairings[0].clone());
    }

    None
  }
  
  fn get_data(&self) -> Option<String> {
    if !self.started {
      return None;
    }
    
    let mut message = String::new();
    for round in self.data.iter() {
      for player in round.iter() {
        message += player;
        message += ",";
      }
      message.pop();
      message.push('|');
    }
    
    if self.data.len() > 0 {
      message.pop();
    }

    Some(message)
  }

  fn get_type(&self) -> String {
    "KnockoutBracket".to_string()
  }
}
