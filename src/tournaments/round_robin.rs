use std::collections::HashMap;

use async_trait::async_trait;

use crate::{server::Server, *};

type Score = u32;
type ID = u32;

#[derive(Clone)]
pub struct RoundRobin {
  pub players: HashMap<ID, (String, Score)>,
  pub top_half: Vec<ID>,
  pub bottom_half: Vec<ID>,
  pub completed: bool,
  pub current_matches: Vec<ID>,
  pub usernames: Vec<String>,
  pub observers: Observers,
}

impl RoundRobin {
  async fn create_matches(&mut self, clients: &Clients, matches: &Matches) {
    let clients_guard = clients.read().await;
    for (i, id) in self.top_half.iter().enumerate() {
      let player1_username = self.players.get(id).unwrap();
      let player2_username = self.players.get(self.bottom_half.get(i).unwrap());

      if player2_username.is_none() {
        continue;
      }
      let player2_addr = player2_username.unwrap();

      let match_id: u32 = gen_match_id(matches).await;
      let new_match = Arc::new(RwLock::new(Match::new(
        match_id,
        player1_username.0.clone(),
        player2_addr.0.clone(),
        false,
      )));

      self.current_matches.push(match_id.clone());
      let match_guard = new_match.read().await;

      let mut player1 = clients_guard.get(&player1_username.0).unwrap().write().await;
      player1.current_match = Some(match_id);
      player1.ready = false;
      broadcast_message(
        &self.observers,
        &format!("READY:{}:{}", player1.username.clone(), false),
      )
      .await;

      if match_guard.player1 == player1_username.0 {
        player1.color = Color::Red;
        let _ = send(&player1.connection, "GAME:START:1");
      } else {
        player1.color = Color::Yellow;
        let _ = send(&player1.connection, "GAME:START:0");
      }
      drop(player1);

      let mut player2 = clients_guard.get(&player2_addr.0).unwrap().write().await;
      player2.current_match = Some(match_id);
      player2.ready = false;
      broadcast_message(
        &self.observers,
        &format!("READY:{}:{}", player2.username.clone(), false),
      )
      .await;

      if match_guard.player1 == player2_addr.0 {
        player2.color = Color::Red;
        let _ = send(&player2.connection, "GAME:START:1");
      } else {
        player2.color = Color::Yellow;
        let _ = send(&player2.connection, "GAME:START:0");
      }
      drop(player2);

      matches.write().await.insert(match_id, new_match.clone());
      broadcast_message(
        &self.observers,
        &format!(
          "GAME:START:{},{},{}",
          match_id, match_guard.player1, match_guard.player2
        ),
      )
      .await;
    }
  }
}

#[async_trait]
impl Tournament for RoundRobin {
  async fn new(ready_players: &[String], server: &Server) -> RoundRobin {
    let mut result = RoundRobin {
      players: HashMap::new(),
      top_half: Vec::new(),
      bottom_half: Vec::new(),
      completed: false,
      current_matches: Vec::new(),
      usernames: ready_players.to_vec(),
      observers: server.observers.clone(),
    };

    let size = ready_players.len();

    for (id, player) in ready_players.iter().enumerate() {
      result.players.insert(id as u32, (player.clone(), 0));
    }

    for i in 0..size / 2 {
      result.top_half.push(i as u32);
    }

    for i in size / 2..size {
      result.bottom_half.push(i as u32);
    }

    result
  }

  async fn inform_winner(&mut self, winner: String, match_id: u32, _: String, _: String) {
    if winner.is_empty() {
      return;
    }

    for (_, username) in self.players.iter_mut() {
      if username.0 == winner {
        username.1 += 1;
        break;
      }
    }

    self.current_matches.retain(|id| !(*id == match_id));
  }

  async fn next(&mut self, server: &Server) {
    if self.completed {
      return;
    }

    if self.top_half.len() <= 1 || self.bottom_half.is_empty() {
      self.completed = true;
      return;
    }

    let last_from_top = self.top_half.pop().unwrap();
    let first_from_bottom = self.bottom_half.remove(0);

    self.top_half.insert(1, first_from_bottom);
    self.bottom_half.push(last_from_top);

    let expected_bottom_start = self.top_half.len() as u32;
    if self.top_half[1] == 1 && self.bottom_half[0] == expected_bottom_start {
      self.completed = true;
    }

    let clients_guard = server.clients.read().await;
    let mut player_scores: Vec<(String, u32)> = Vec::new();
    for (_, player_addr) in self.players.iter() {
      let player = clients_guard.get(&player_addr.0).unwrap().read().await;
      player_scores.push((player.username.clone(), player_addr.1));
    }
    drop(clients_guard);

    player_scores.sort_by(|a, b| b.1.cmp(&a.1));

    // Send scores
    let mut message = "TOURNAMENT:SCORES:".to_string();
    for (player, score) in player_scores.iter() {
      message.push_str(&format!("{},{}|", player, score))
    }
    message.pop();

    server.broadcast(&message).await;

    if !self.is_completed() {
      self.create_matches(&server.clients, &server.matches).await;
    }
  }

  async fn start(&mut self, server: &Server) {
    self.create_matches(&server.clients, &server.matches).await;
  }

  async fn cancel(&mut self, server: &Server) {
    for match_id in &self.current_matches {
      server.terminate_match(*match_id).await;
    }

    let clients_guard = server.clients.read().await;
    for (_, (username, _)) in self.players.iter() {
      let client = clients_guard.get(username);
      if client.is_none() {
        continue;
      }

      let client = client.unwrap().read().await;
      let _ = send(&client.connection, "TOURNAMENT:END");
    }
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
    if !self.is_completed() {
      return None;
    }

    let mut best_score = 0;
    let mut winner = None;

    for (_, (username, score)) in self.players.iter() {
      if *score > best_score {
        best_score = *score;
        winner = Some(username.clone());
      }
    }

    winner
  }

  fn get_data(&self) -> Option<String> {
    None
  }

  fn get_type(&self) -> String {
    "RoundRobin".to_string()
  }
}
