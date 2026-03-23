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
  pub is_completed: bool,
}

impl RoundRobin {
  async fn create_matches(&self, clients: &Clients, matches: &Matches) {
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

      let match_guard = new_match.read().await;
      let mut player1 = clients_guard.get(&player1_username.0).unwrap().write().await;

      player1.current_match = Some(match_id);
      player1.ready = false;

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

      if match_guard.player1 == player2_addr.0 {
        player2.color = Color::Red;
        let _ = send(&player2.connection, "GAME:START:1");
      } else {
        player2.color = Color::Yellow;
        let _ = send(&player2.connection, "GAME:START:0");
      }

      drop(player2);

      matches.write().await.insert(match_id, new_match.clone());
    }
  }
}

#[async_trait]
impl Tournament for RoundRobin {
  fn new(ready_players: &[String]) -> RoundRobin {
    let mut result = RoundRobin {
      players: HashMap::new(),
      top_half: Vec::new(),
      bottom_half: Vec::new(),
      is_completed: false,
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

  fn inform_winner(&mut self, winner: String, is_tie: bool) {
    if is_tie {
      return;
    }

    for (_, username) in self.players.iter_mut() {
      if username.0 == winner {
        username.1 += 1;
        break;
      }
    }
  }

  fn contains_player(&self, username: String) -> bool {
    for (_, (player_username, _)) in self.players.iter() {
      if *player_username == username {
        return true;
      }
    }
    false
  }

  async fn next(&mut self, server: &Server) {
    if self.is_completed {
      return;
    }

    if self.top_half.len() <= 1 || self.bottom_half.is_empty() {
      self.is_completed = true;
      return;
    }

    let last_from_top = self.top_half.pop().unwrap();
    let first_from_bottom = self.bottom_half.remove(0);

    self.top_half.insert(1, first_from_bottom);
    self.bottom_half.push(last_from_top);

    let expected_bottom_start = self.top_half.len() as u32;
    if self.top_half[1] == 1 && self.bottom_half[0] == expected_bottom_start {
      self.is_completed = true;
    }

    let clients_guard = server.clients.read().await;
    let mut player_scores: Vec<(String, u32)> = Vec::new();
    for (_, player_addr) in self.players.iter() {
      let player = clients_guard.get(&player_addr.0).unwrap().read().await;
      let _ = send(&player.connection.clone(), "TOURNAMENT:END");
      player_scores.push((player.username.clone(), player_addr.1));
    }
    drop(clients_guard);

    player_scores.sort_by(|a, b| b.1.cmp(&a.1));

    let mut message = "TOURNAMENT:SCORES:".to_string();
    for (player, score) in player_scores.iter() {
      message.push_str(&format!("{},{}|", player, score))
    }
    message.pop();

    server.broadcast(&message).await;

    if self.is_completed() {
      // Send scores
      let clients_guard = server.clients.read().await;
      for (_, player_addr) in self.players.iter() {
        let player = clients_guard.get(&player_addr.0).unwrap().read().await;
        let _ = send(&player.connection.clone(), "TOURNAMENT:END");
      }
    } else {
      // Create next matches
      self.create_matches(&server.clients, &server.matches).await;
    }
  }

  async fn start(&mut self, server: &Server) {
    self.create_matches(&server.clients, &server.matches).await;
  }

  async fn cancel(&mut self, server: &Server) {
    for (_, addr) in self.players.iter() {
      let clients_guard = server.clients.read().await;

      let client = clients_guard.get(&addr.0);
      if client.is_none() {
        continue;
      }
      let client = client.unwrap().read().await;
      let client_connection = client.connection.clone();
      let client_ready = client.ready;

      let match_id = client.current_match;
      if match_id.is_none() {
        continue;
      }
      let match_id = match_id.unwrap();

      drop(client);
      drop(clients_guard);

      server.terminate_match(match_id).await;

      if !client_ready {
        let _ = send(&client_connection, "TOURNAMENT:END");
      }
    }
  }

  fn is_completed(&self) -> bool {
    self.is_completed
  }

  fn get_type(&self) -> String {
    "RoundRobin".to_string()
  }
}
