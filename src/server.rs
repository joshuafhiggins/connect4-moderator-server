use anyhow::anyhow;
use std::time::Instant;

use crate::{tournaments::*, types::*, *};

pub struct Server {
  pub clients: Clients,
  pub disconnected_clients: Arc<RwLock<Vec<String>>>,
  pub usernames: Usernames,
  pub observers: Observers,
  pub matches: Matches,
  pub reservations: Reservations,
  pub admin: Arc<RwLock<Option<SocketAddr>>>,
  pub admin_password: Arc<String>,
  pub tournament: WrappedTournament,
  pub waiting_timeout: Arc<RwLock<u64>>,
  pub max_timeout: Arc<RwLock<u64>>,
  pub demo_mode: Arc<RwLock<bool>>,
}

impl Server {
  pub fn new(admin_password: String, demo_mode: bool) -> Server {
    Server {
      clients: Arc::new(RwLock::new(HashMap::new())),
      disconnected_clients: Arc::new(RwLock::new(Vec::new())),
      usernames: Arc::new(RwLock::new(HashMap::new())),
      observers: Arc::new(RwLock::new(HashMap::new())),
      matches: Arc::new(RwLock::new(HashMap::new())),
      reservations: Arc::new(RwLock::new(Vec::new())),
      admin: Arc::new(RwLock::new(None)),
      admin_password: Arc::new(admin_password),
      tournament: Arc::new(RwLock::new(None)),
      waiting_timeout: Arc::new(RwLock::new(5000)),
      max_timeout: Arc::new(RwLock::new(30000)),
      demo_mode: Arc::new(RwLock::new(demo_mode)),
    }
  }

  pub async fn handle_connect_cmd(
    &self,
    addr: SocketAddr,
    tx: UnboundedSender<Message>,
    requested_username: String,
  ) -> Result<(), anyhow::Error> {
    if requested_username.is_empty() {
      return Err(anyhow::anyhow!(format!(
        "ERROR:INVALID:CONNECT:{}",
        requested_username
      )));
    }

    if requested_username.chars().any(|c| matches!(c, ':' | ',' | '|') || c.is_control()) {
      return Err(anyhow::anyhow!(format!(
        "ERROR:INVALID:CONNECT:{}",
        requested_username
      )));
    }

    if requested_username == SERVER_PLAYER_USERNAME {
      return Err(anyhow::anyhow!(format!(
        "ERROR:INVALID:CONNECT:{}",
        requested_username
      )));
    }

    let reconnecting = self.disconnected_clients.read().await.contains(&requested_username);

    let clients_guard = self.clients.read().await;
    let existing_client = clients_guard.get(&requested_username).cloned();
    if existing_client.is_some() && !reconnecting {
      return Err(anyhow::anyhow!(format!(
        "ERROR:INVALID:CONNECT:{}",
        requested_username
      )));
    }

    drop(clients_guard);

    self.observers.write().await.remove(&addr);
    self.usernames.write().await.insert(addr, requested_username.clone());
    let _ = send(&tx, "CONNECT:ACK");

    if !reconnecting {
      self.clients.write().await.insert(
        requested_username.clone(),
        Arc::new(RwLock::new(Client::new(
          requested_username.clone(),
          tx.clone(),
          addr.to_string().parse()?,
        ))),
      );

      self.broadcast(&format!("CONNECT:{}", requested_username)).await;

      return Ok(());
    }

    // reconnecting
    self.disconnected_clients.write().await.retain(|name| name != &requested_username);
    let client_guard = existing_client.unwrap();
    let mut client = client_guard.write().await;
    client.addr = addr;
    client.connection = tx.clone();

    if let Some(current_match_id) = client.current_match {
      let current_match = self.matches.read().await.get(&current_match_id).cloned().unwrap();
      let mut current_match = current_match.write().await;
      current_match.ledger.clear();
      current_match.board = vec![vec![Color::None; 6]; 7];
      let opponent_username = if current_match.player1 == requested_username {
        current_match.player2.clone()
      } else {
        current_match.player1.clone()
      };

      let clients_guard = self.clients.read().await;
      let opponent = clients_guard.get(&opponent_username).unwrap().read().await;
      let _ = send(&opponent.connection, "GAME:TERMINATED");
      let _ = send(
        &tx,
        &format!("GAME:START:{}", bool::from(client.color) as u8),
      );
      let _ = send(
        &opponent.connection,
        &format!("GAME:START:{}", bool::from(opponent.color) as u8),
      );
    }

    Ok(())
  }

  pub async fn handle_reconnect_cmd(
    &self,
    addr: SocketAddr,
    tx: UnboundedSender<Message>,
    requested_username: String,
  ) -> Result<(), anyhow::Error> {
    let clients_guard = self.clients.read().await;
    let disconnected_guard = self.disconnected_clients.read().await;
    let found_client = if disconnected_guard.contains(&requested_username) {
      clients_guard.get(&requested_username).cloned()
    } else {
      None
    };

    drop(clients_guard);
    drop(disconnected_guard);

    if let Some(client_guard) = found_client {
      self.disconnected_clients.write().await.retain(|name| name != &requested_username);
      self.usernames.write().await.insert(addr, requested_username.clone());
      let mut client = client_guard.write().await;
      client.addr = addr;
      client.connection = tx.clone();

      if let Some(current_match_id) = client.current_match {
        let matches_guard = self.matches.read().await;
        let the_match = matches_guard.get(&current_match_id).unwrap().read().await;

        let last = the_match.ledger.last();
        if last.is_some() && last.unwrap().0 != client.color {
          let _ = send(
            &tx,
            &format!("OPPONENT:{}", the_match.ledger.last().unwrap().1),
          );
        } else if last.is_none() && client.color == Color::Red {
          let _ = send(&tx, "GAME:START:1");
        }
      } else {
        // Clear they're state just in case, even if it's not terminated
        let _ = send(&tx, "GAME:TERMINATED");
      }

      let tournament = self.tournament.read().await.as_ref().cloned();
      if let Some(tourney) = tournament {
        let tourney = tourney.read().await;
        if !tourney.contains_player(requested_username) {
          let _ = send(&tx, "RECONNECT:ACK");
        }
      }
    } else {
      // return Err(anyhow::anyhow!(format!(
      //   "ERROR:INVALID:RECONNECT:{}",
      //   requested_username
      // )));
    }

    Ok(())
  }

  pub async fn handle_disconnect_cmd(
    &self,
    addr: SocketAddr,
    tx: UnboundedSender<Message>,
  ) -> Result<(), anyhow::Error> {
    let clients_guard = self.clients.read().await;
    let usernames_guard = self.usernames.read().await;
    let username = usernames_guard.get(&addr).cloned();

    if username.is_none() {
      return Err(anyhow::anyhow!("ERROR:INVALID:DISCONNECT"));
    }

    let username = username.unwrap();
    let client = clients_guard.get(&username).cloned().unwrap();
    let client = client.read().await;

    drop(clients_guard);
    drop(usernames_guard);

    if client.current_match.is_some() {
      let match_id = client.current_match.unwrap();
      drop(client);

      self.terminate_match(match_id).await;
    }

    self.clients.write().await.remove(&username);
    self.usernames.write().await.remove(&addr);
    let _ = send(&tx, "DISCONNECT:ACK");
    self.broadcast(&format!("DISCONNECT:{}", username)).await;

    Ok(())
  }

  pub async fn handle_observe(
    &self,
    addr: SocketAddr,
    tx: UnboundedSender<Message>,
  ) -> Result<(), anyhow::Error> {
    if self.usernames.read().await.get(&addr).cloned().is_some() {
      return Err(anyhow::anyhow!("ERROR:INVALID:OBSERVE"));
    }

    let mut observers_guard = self.observers.write().await;
    if observers_guard.remove(&addr).is_some() {
      let _ = send(&tx, "OBSERVE:ACK:0");
    } else {
      observers_guard.insert(addr, tx.clone());
      let _ = send(&tx, "OBSERVE:ACK:1");
    }

    Ok(())
  }

  pub async fn handle_ready(
    &self,
    addr: SocketAddr,
    tx: UnboundedSender<Message>,
  ) -> Result<(), anyhow::Error> {
    let clients_guard = self.clients.read().await;
    let username = self.usernames.read().await.get(&addr).cloned();

    if username.is_none() {
      return Err(anyhow::anyhow!("ERROR:INVALID"));
    }

    let username = username.unwrap();

    if clients_guard.get(&username).unwrap().read().await.ready {
      let _ = send(&tx, "READY:ACK");
      return Ok(());
    }

    let mut client = clients_guard.get(&username).unwrap().write().await;
    client.ready = true;
    let _ = send(&tx, "READY:ACK");
    drop(client);
    drop(clients_guard);
    self.broadcast(&format!("READY:{}:{}", username, true)).await;

    if let Some(opponent_username) = self.find_reservation_opponent(&username).await {
      let clients_guard = self.clients.read().await;
      let mut client = clients_guard.get(&username).unwrap().write().await;
      let mut opponent = clients_guard.get(&opponent_username).unwrap().write().await;

      if opponent.ready {
        let match_id: u32 = gen_match_id(&self.matches).await;
        let new_match = Arc::new(RwLock::new(Match::new(
          match_id,
          username.clone(),
          opponent_username,
          false,
        )));
        self.matches.write().await.insert(match_id, new_match.clone());
        let match_guard = new_match.read().await;
        self
          .broadcast(&format!(
            "GAME:START:{},{},{}",
            match_id, match_guard.player1, match_guard.player2
          ))
          .await;
        drop(match_guard);

        client.ready = false;
        client.current_match = Some(match_id);
        client.color = if new_match.read().await.player1 == username {
          let _ = send(&tx, "GAME:START:1");
          let _ = send(&opponent.connection, "GAME:START:0");
          Color::Red
        } else {
          let _ = send(&tx, "GAME:START:0");
          let _ = send(&opponent.connection, "GAME:START:1");
          Color::Yellow
        };

        opponent.ready = false;
        opponent.current_match = Some(match_id);
        opponent.color = !client.color;
        let opponent_username = opponent.username.clone();

        self
          .reservations
          .write()
          .await
          .retain(|(p1, p2)| !(p1 == &client.username && p2 == &opponent.username));

        drop(opponent);
        drop(client);
        drop(clients_guard);
        self.broadcast(&format!("READY:{}:{}", username, false)).await;
        self.broadcast(&format!("READY:{}:{}", opponent_username, false)).await;

        return Ok(());
      }
    }

    let clients_guard = self.clients.read().await;
    let mut client = clients_guard.get(&username).unwrap().write().await;
    let is_demo_mode = self.demo_mode.read().await.clone();
    if is_demo_mode {
      let match_id: u32 = gen_match_id(&self.matches).await;
      let new_match = Arc::new(RwLock::new(Match::new(
        match_id,
        username.clone(),
        SERVER_PLAYER_USERNAME.to_string(),
        is_demo_mode,
      )));
      self.matches.write().await.insert(match_id, new_match.clone());
      let match_guard = new_match.read().await;
      self
        .broadcast(&format!(
          "GAME:START:{},{},{}",
          match_id, match_guard.player1, match_guard.player2
        ))
        .await;
      drop(match_guard);

      client.ready = false;
      client.current_match = Some(match_id);
      client.color = if new_match.read().await.player1 == username {
        let _ = send(&tx, "GAME:START:1");
        Color::Red
      } else {
        let _ = send(&tx, "GAME:START:0");
        Color::Yellow
      };

      if client.color == Color::Yellow {
        let default_waiting_time = *self.waiting_timeout.read().await;

        let demo_move = {
          let mut the_match = new_match.write().await;
          let opening_move = random_move(&the_match.board);
          the_match.ledger.push((Color::Red, opening_move, Instant::now()));
          the_match.place_token(Color::Red, opening_move);
          opening_move
        };

        let observers = self.observers.clone();
        let client_tx = tx.clone();
        let wait_match_id = match_id;

        new_match.write().await.wait_thread = Some(tokio::spawn(async move {
          tokio::time::sleep(tokio::time::Duration::from_millis(default_waiting_time)).await;
          let _ = send(&client_tx, &format!("OPPONENT:{}", demo_move));

          let observers_guard = observers.read().await;
          let msg = format!(
            "GAME:{}:MOVE:{}:{}",
            wait_match_id, SERVER_PLAYER_USERNAME, demo_move
          );
          for (_, tx) in observers_guard.iter() {
            let _ = send(tx, &msg);
          }
        }));
      }

      drop(client);
      self.broadcast(&format!("READY:{}:{}", username, false)).await;
    }

    Ok(())
  }

  pub async fn handle_play(
    &self,
    addr: SocketAddr,
    tx: UnboundedSender<Message>,
    column: usize,
  ) -> Result<(), anyhow::Error> {
    let clients_guard = self.clients.read().await;
    let usernames_guard = self.usernames.read().await;
    let username = usernames_guard.get(&addr).cloned();

    if username.is_none() {
      return Err(anyhow::anyhow!("ERROR:INVALID:MOVE"));
    }

    let username = username.unwrap();
    let client = clients_guard.get(&username).unwrap().read().await;

    // Check if client is valid
    if client.current_match.is_none() {
      return Err(anyhow::anyhow!("ERROR:INVALID:MOVE"));
    }

    let matches_guard = self.matches.read().await;
    let current_match = matches_guard.get(&client.current_match.unwrap()).unwrap().read().await;

    let opponent = {
      let mut result = None;

      if !current_match.demo_mode {
        let opponent_username = if username == current_match.player1 {
          current_match.player2.clone()
        } else {
          current_match.player1.clone()
        };

        result = Some(clients_guard.get(&opponent_username).cloned().unwrap());
      }

      result
    };

    // Check if it's their move
    let mut invalid = false;
    if (current_match.ledger.is_empty() && current_match.player1 != username)
      || (current_match.ledger.last().is_some()
        && current_match.ledger.last().unwrap().0 == client.color)
    {
      let _ = send(&tx, "ERROR:INVALID:MOVE");
      invalid = true;
    }

    drop(current_match);
    drop(matches_guard);

    let mut matches_guard = self.matches.write().await;
    let mut current_match =
      matches_guard.get_mut(&client.current_match.unwrap()).unwrap().write().await;

    // Check if valid move
    if column >= 7 && !invalid {
      let _ = send(&tx, "ERROR:INVALID:MOVE");
      invalid = true;
    }

    if current_match.board[column][5] != Color::None && !invalid {
      let _ = send(&tx, "ERROR:INVALID:MOVE");
      invalid = true;
    }

    let player1 = current_match.player1.clone();
    let player2 = current_match.player2.clone();

    // Terminate games if a player makes an invalid move
    if invalid {
      let current_match_id = current_match.id;
      let is_demo_mode = current_match.demo_mode;

      drop(current_match);
      drop(matches_guard);
      drop(client);
      drop(clients_guard);

      if is_demo_mode {
        self.terminate_match(current_match_id).await;
        tx.send(Message::Close(None))?;
      } else {
        let opponent = opponent.unwrap();
        let mut opponent = opponent.write().await;

        let _ = send(&tx, "GAME:LOSS");
        let _ = send(&opponent.connection, "GAME:WINS");
        self
          .broadcast(&format!(
            "GAME:{}:WIN:{}",
            current_match_id, opponent.username
          ))
          .await;

        opponent.current_match = None;
        opponent.color = Color::None;
        let opponent_username = opponent.username.clone();
        drop(opponent);

        let mut clients_guard = self.clients.write().await;
        let mut client = clients_guard.get_mut(&username).unwrap().write().await;
        client.current_match = None;
        client.color = Color::None;
        drop(client);

        self.matches.write().await.remove(&current_match_id).unwrap();

        let mut tournament_guard = self.tournament.write().await;
        let tourney = tournament_guard.as_mut().unwrap();
        tourney
          .write()
          .await
          .inform_winner(
            opponent_username,
            current_match_id,
            player1.clone(),
            player2.clone(),
          )
          .await;
        drop(tournament_guard);
      }
      return Ok(());
    }

    current_match.place_token(client.color.clone(), column);

    if let Some(timeout_thread) = &current_match.timeout_thread {
      timeout_thread.abort();
    }

    let mut observer_messages = Vec::new();
    observer_messages.push(format!(
      "GAME:{}:MOVE:{}:{}",
      current_match.id, client.username, column
    ));

    // Check game end conditions
    let (winner, filled) = current_match.end_game_check();

    // Send match results
    if winner == client.color {
      let _ = send(&tx, "GAME:WINS");
      if !current_match.demo_mode {
        let opponent = opponent.clone().unwrap();
        let opponent = opponent.read().await;
        let _ = send(&opponent.connection, "GAME:LOSS");
      }
      observer_messages.push(format!("GAME:{}:WIN:{}", current_match.id, client.username));
    } else if filled {
      let _ = send(&tx, "GAME:DRAW");
      if !current_match.demo_mode {
        let opponent = opponent.clone().unwrap();
        let opponent = opponent.read().await;
        let _ = send(&opponent.connection, "GAME:DRAW");
      }
      observer_messages.push(format!("GAME:{}:DRAW", current_match.id));
    }

    // remove match from matchmaker
    if winner != Color::None || filled {
      for msg in observer_messages {
        self.broadcast(&msg).await;
      }

      let current_match_id = current_match.id;
      let is_demo_mode = current_match.demo_mode;

      drop(client);
      drop(current_match);
      drop(clients_guard);

      let clients_guard = self.clients.read().await;
      let mut client = clients_guard.get(&username).unwrap().write().await;
      client.current_match = None;
      client.color = Color::None;
      drop(client);

      if !is_demo_mode {
        let opponent = opponent.clone().unwrap();
        let mut opponent = opponent.write().await;
        opponent.current_match = None;
        opponent.color = Color::None;
        drop(opponent);
      }

      matches_guard.remove(&current_match_id).unwrap();
      drop(matches_guard);

      if self.tournament.read().await.is_some() {
        let mut tournament_guard = self.tournament.write().await;
        let tourney = tournament_guard.as_mut().unwrap();
        let winner = if filled {
          String::new()
        } else {
          username.clone()
        };

        tourney
          .write()
          .await
          .inform_winner(winner, current_match_id, player1.clone(), player2.clone())
          .await;

        if self.matches.read().await.is_empty() {
          drop(clients_guard);

          tourney.write().await.next(&self).await;
          if tourney.read().await.is_completed() {
            let tournament_players = tourney.read().await.get_players();
            let clients_guard = self.clients.read().await;

            for player in tournament_players {
              let player = clients_guard.get(&player);

              if player.is_none() {
                continue;
              }

              let player = player.unwrap().read().await;
              let _ = send(&player.connection, "TOURNAMENT:END");
            }

            let tournament_winner = tourney.read().await.get_winner().unwrap();
            *tournament_guard = None;

            self.broadcast("TOURNAMENT:END").await;
            self.broadcast(&format!("TOURNAMENT:WINNER:{}", tournament_winner)).await;
          }
        }
      }

      return Ok(());
    }

    let set_waiting_time = *self.waiting_timeout.read().await;
    let mut variance = rand::rng().random_range(0..=(set_waiting_time / 200)) as i64;
    variance *= rand::rng().random_range(0..=2) - 1;
    let mut adjusted_waiting = set_waiting_time as i64 + variance;
    let current_move_time = Instant::now();

    if current_match.ledger.is_empty() {
      adjusted_waiting = 0;
    } else {
      let last_move_time = current_match.ledger.last().unwrap().2;
      let elapsed = current_move_time.duration_since(last_move_time).as_millis() as i64;
      adjusted_waiting -= elapsed;
      if adjusted_waiting < 0 {
        adjusted_waiting = 0;
      }
    }

    current_match.ledger.push((client.color.clone(), column, current_move_time));

    let demo_mode = current_match.demo_mode;
    let demo_move = random_move(&current_match.board);
    let no_winner = winner == Color::None && !filled;
    let observers = self.observers.clone();
    let opponent_move = opponent.clone();
    let client_tx = tx.clone();
    let wait_match_id = current_match.id;
    if current_match.demo_mode {
      current_match.ledger.push((!client.color, demo_move, Instant::now()));
      current_match.place_token(!client.color, demo_move);
    }

    current_match.wait_thread = Some(tokio::spawn(async move {
      tokio::time::sleep(tokio::time::Duration::from_millis(adjusted_waiting as u64)).await;

      if !demo_mode && no_winner {
        let opponent = opponent_move.unwrap();
        let opponent = opponent.read().await;
        let _ = send(
          &opponent.connection.clone(),
          &format!("OPPONENT:{}", column),
        );
      }

      for msg in observer_messages {
        broadcast_message(&observers, &msg).await;
      }

      if demo_mode && no_winner {
        tokio::time::sleep(tokio::time::Duration::from_millis(set_waiting_time)).await;
        let _ = send(&client_tx, &format!("OPPONENT:{}", demo_move));
        let observers_guard = observers.read().await;
        let msg = format!(
          "GAME:{}:MOVE:{}:{}",
          wait_match_id, SERVER_PLAYER_USERNAME, demo_move
        );
        for (_, tx) in observers_guard.iter() {
          let _ = send(tx, &msg);
        }
      }
    }));

    let max_timeout = *self.max_timeout.read().await;
    let matches = self.matches.clone();
    let tournament = self.tournament.clone();
    let clients = self.clients.clone();
    let match_id = current_match.id;
    let ledger_size = current_match.ledger.len();
    let client_username = client.username.clone();
    let client_tx = tx.clone();
    let observers = self.observers.clone();
    let opponent_move = opponent.clone();
    current_match.timeout_thread = Some(tokio::spawn(async move {
      if demo_mode {
        return;
      }
      tokio::time::sleep(tokio::time::Duration::from_millis(adjusted_waiting as u64)).await;
      tokio::time::sleep(tokio::time::Duration::from_millis(max_timeout as u64)).await;

      let matches_guard = matches.read().await;
      let the_match = matches_guard.get(&match_id).cloned();
      drop(matches_guard);
      if let Some(the_match) = the_match {
        let the_match = the_match.read().await;
        if the_match.ledger.len() == ledger_size {
          // forfeit the match
          let _ = send(&client_tx, "GAME:WINS");
          let opponent = opponent_move.clone().unwrap();
          let opponent = opponent.read().await;
          let _ = send(&opponent.connection, "GAME:LOSS");
          drop(opponent);
          let observers_guard = observers.read().await;
          let msg = format!("GAME:{}:WIN:{}", match_id, client_username);
          for (_, tx) in observers_guard.iter() {
            let _ = send(tx, &msg);
          }

          let mut clients_guard = clients.write().await;
          let mut client = clients_guard.get_mut(&client_username).unwrap().write().await;
          client.current_match = None;
          client.color = Color::None;
          drop(client);

          let opponent = opponent_move.unwrap();
          let mut opponent = opponent.write().await;
          opponent.current_match = None;
          opponent.color = Color::None;
          drop(opponent);

          matches.write().await.remove(&match_id).unwrap();

          let mut tournament_guard = tournament.write().await;
          let tourney = tournament_guard.as_mut().unwrap();
          tourney.write().await.inform_winner(client_username, match_id, player1, player2).await;
          drop(tournament_guard);
        }
      }
    }));

    Ok(())
  }

  pub async fn handle_player_list(
    &self,
    tx: UnboundedSender<Message>,
  ) -> Result<(), anyhow::Error> {
    let clients_guard = self.clients.read().await;
    let mut to_send = "PLAYER:LIST:".to_string();
    for client_guard in clients_guard.values() {
      let player = client_guard.read().await;
      to_send += player.username.as_str();
      to_send += ",";
      to_send += if player.ready { "true" } else { "false" };
      to_send += ",";
      to_send += if player.current_match.is_some() {
        "true"
      } else {
        "false"
      };
      to_send += "|";
    }

    if !to_send.ends_with(":") {
      to_send.remove(to_send.len() - 1);
    }

    let _ = send(&tx, to_send.as_str());
    Ok(())
  }

  pub async fn handle_game_list(&self, tx: UnboundedSender<Message>) -> Result<(), anyhow::Error> {
    let matches_guard = self.matches.read().await;
    let mut to_send = "GAME:LIST:".to_string();
    for match_guard in matches_guard.values() {
      let a_match = match_guard.read().await;
      to_send += a_match.id.to_string().as_str();
      to_send += ",";
      to_send += a_match.player1.as_str();
      to_send += ",";
      to_send += a_match.player2.as_str();
      to_send += "|";
    }

    if !to_send.ends_with(":") {
      to_send.remove(to_send.len() - 1);
    }

    let _ = send(&tx, to_send.as_str());
    Ok(())
  }

  pub async fn handle_game_watch(
    &self,
    tx: UnboundedSender<Message>,
    match_id: u32,
    _addr: SocketAddr,
  ) -> Result<(), anyhow::Error> {
    let matches_guard = self.matches.read().await;
    let the_match = matches_guard.get(&match_id).unwrap().read().await;

    let player1 = the_match.player1.clone();
    let player2 = the_match.player2.clone();

    let ledger = the_match.ledger.clone();

    drop(the_match);
    drop(matches_guard);

    let mut message = format!("GAME:WATCH:ACK:{},{},{}|", match_id, player1, player2);

    for a_move in ledger {
      if a_move.0 == Color::Red {
        message += &format!("{},{}|", player1, a_move.1);
      } else {
        message += &format!("{},{}|", player2, a_move.1);
      }
    }

    message.pop();

    let _ = send(&tx, &message);
    Ok(())
  }

  pub async fn handle_admin_auth(
    &self,
    tx: UnboundedSender<Message>,
    addr: SocketAddr,
    password: String,
  ) -> Result<(), anyhow::Error> {
    if self.admin.read().await.is_some() {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    if password != *self.admin_password {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    let mut admin_guard = self.admin.write().await;
    *admin_guard = Some(addr.to_string().parse()?);
    let _ = send(&tx, "ADMIN:AUTH:ACK");
    Ok(())
  }

  pub async fn handle_admin_kick(
    &self,
    addr: SocketAddr,
    kick_username: String,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    let clients_guard = self.clients.read().await;

    let kick_client = clients_guard.get(&kick_username).cloned();
    if let Some(client) = kick_client {
      client.read().await.connection.send(Message::Close(None))?;
    } else {
      return Err(anyhow::anyhow!("ERROR:INVALID:KICK"));
    }

    Ok(())
  }

  pub async fn handle_game_terminate(
    &self,
    addr: SocketAddr,
    match_id: u32,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    self.terminate_match(match_id).await;

    if self.tournament.read().await.is_some() && self.matches.read().await.is_empty() {
      let mut tournament_guard = self.tournament.write().await;
      let tourney = tournament_guard.as_mut().unwrap();
      tourney.write().await.next(&self).await;
      if tourney.read().await.is_completed() {
        let tournament_players = tourney.read().await.get_players();
        let clients_guard = self.clients.read().await;

        for player in tournament_players {
          let player = clients_guard.get(&player);

          if player.is_none() {
            continue;
          }

          let player = player.unwrap().read().await;
          let _ = send(&player.connection, "TOURNAMENT:END");
        }

        let tournament_winner = tourney.read().await.get_winner().unwrap();
        *tournament_guard = None;

        self.broadcast("TOURNAMENT:END").await;
        self.broadcast(&format!("TOURNAMENT:WINNER:{}", tournament_winner)).await;
      }
    }
    Ok(())
  }

  pub async fn handle_game_award_winner(
    &self,
    addr: SocketAddr,
    match_id: u32,
    winner_username: String,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow!("ERROR:INVALID:AUTH"));
    }

    let matches_guard = self.matches.read().await;
    let found_match =
      matches_guard.get(&match_id).ok_or_else(|| anyhow!("ERROR:INVALID:AWARD"))?.clone();
    drop(matches_guard);

    let the_match = found_match.read().await;

    // Validate that the declared winner is actually one of the players in this match
    let clients_guard = self.clients.read().await;
    let player1_client = clients_guard.get(&the_match.player1);
    let player2_client = clients_guard.get(&the_match.player2);

    // If we cannot resolve both players, or the winner username doesn't match either, reject
    if let (Some(p1_arc), Some(p2_arc)) = (player1_client, player2_client) {
      let p1 = p1_arc.read().await;
      let p2 = p2_arc.read().await;

      if winner_username != p1.username && winner_username != p2.username {
        return Err(anyhow!("ERROR:INVALID:AWARD"));
      }
    } else {
      return Err(anyhow!("ERROR:INVALID:AWARD"));
    }
    drop(clients_guard);

    self.matches.write().await.remove(&match_id);

    if let Some(wait_thread) = &the_match.wait_thread {
      wait_thread.abort();
    }

    if let Some(timeout_thread) = &the_match.timeout_thread {
      timeout_thread.abort();
    }

    self.broadcast(&format!("GAME:{}:WIN:{}", match_id, winner_username)).await;

    let clients_guard = self.clients.read().await;
    if the_match.demo_mode {
      let player_win = if winner_username != SERVER_PLAYER_USERNAME {
        "WINS"
      } else {
        "LOSS"
      };
      let mut the_player = if the_match.player1 != SERVER_PLAYER_USERNAME {
        clients_guard.get(&the_match.player1).unwrap().write().await
      } else {
        clients_guard.get(&the_match.player2).unwrap().write().await
      };

      let _ = send(&the_player.connection, &format!("GAME:{}", player_win));
      let _ = send(&the_player.connection, "TOURNAMENT:END");

      the_player.color = Color::None;
      the_player.current_match = None;

      return Ok(());
    }

    let mut player1 = clients_guard.get(&the_match.player1).unwrap().write().await;
    let mut player2 = clients_guard.get(&the_match.player2).unwrap().write().await;

    player1.current_match = None;
    player1.color = Color::None;

    player2.current_match = None;
    player2.color = Color::None;

    let winner_tx = if player1.username == winner_username {
      player1.connection.clone()
    } else {
      player2.connection.clone()
    };

    let loser_tx = if player1.username != winner_username {
      player1.connection.clone()
    } else {
      player2.connection.clone()
    };

    drop(player1);
    drop(player2);
    drop(clients_guard);

    let _ = send(&winner_tx, "GAME:WINS");
    let _ = send(&loser_tx, "GAME:LOSS");

    if self.tournament.read().await.is_some() {
      let mut tournament_guard = self.tournament.write().await;
      let tourney = tournament_guard.as_mut().unwrap();
      tourney
        .write()
        .await
        .inform_winner(
          winner_username,
          match_id,
          the_match.player1.clone(),
          the_match.player2.clone(),
        )
        .await;
      if self.matches.read().await.is_empty() {
        tourney.write().await.next(&self).await;
        if tourney.read().await.is_completed() {
          let tournament_players = tourney.read().await.get_players();
          let clients_guard = self.clients.read().await;

          for player in tournament_players {
            let player = clients_guard.get(&player);

            if player.is_none() {
              continue;
            }

            let player = player.unwrap().read().await;
            let _ = send(&player.connection, "TOURNAMENT:END");
          }

          let tournament_winner = tourney.read().await.get_winner().unwrap();
          *tournament_guard = None;

          self.broadcast("TOURNAMENT:END").await;
          self.broadcast(&format!("TOURNAMENT:WINNER:{}", tournament_winner)).await;
        }
      }
    } else {
      let _ = send(&winner_tx, "TOURNAMENT:END");
      let _ = send(&loser_tx, "TOURNAMENT:END");
    }

    Ok(())
  }

  pub async fn handle_tournament_start(
    &self,
    addr: SocketAddr,
    tournament_type: String,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    if self.tournament.read().await.is_some() {
      return Err(anyhow::anyhow!("ERROR:INVALID:TOURNAMENT"));
    }

    let mut clients_guard = self.clients.write().await;
    let mut ready_players = Vec::new();
    for (username, client_guard) in clients_guard.iter_mut() {
      if client_guard.read().await.ready {
        ready_players.push(username.clone());
      }
    }

    if ready_players.len() < 3 {
      return Err(anyhow::anyhow!("ERROR:INVALID:TOURNAMENT"));
    }

    drop(clients_guard);

    let tourney: Option<Arc<RwLock<dyn Tournament + Send + Sync + 'static>>> =
      match tournament_type.as_str() {
        "RoundRobin" => Some(Arc::new(RwLock::new(
          RoundRobin::new(&ready_players, &self).await,
        ))),
        "KnockoutBracket" => Some(Arc::new(RwLock::new(
          KnockoutBracket::new(&ready_players, &self).await,
        ))),
        &_ => None,
      };

    if tourney.is_none() {
      return Err(anyhow::anyhow!("ERROR:INVALID:TOURNAMENT"));
    }

    let tourney = tourney.unwrap();
    tourney.write().await.start(&self).await;

    let mut tournament_guard = self.tournament.write().await;
    *tournament_guard = Some(tourney);

    // Clear any pending reservations when a tournament starts
    self.reservations.write().await.clear();

    self.broadcast(&format!("TOURNAMENT:START:{}", tournament_type)).await;
    Ok(())
  }

  pub async fn handle_tournament_cancel(&self, addr: SocketAddr) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    if self.tournament.read().await.is_none() {
      return Err(anyhow::anyhow!("ERROR:INVALID:TOURNAMENT"));
    }

    let mut tournament_guard = self.tournament.write().await;
    let tourney = tournament_guard.as_mut().unwrap();
    tourney.write().await.cancel(&self).await;
    *tournament_guard = None;

    self.broadcast("TOURNAMENT:CANCEL").await;
    Ok(())
  }

  pub async fn handle_get_data(
    &self,
    tx: UnboundedSender<Message>,
    data_id: String,
  ) -> Result<(), anyhow::Error> {
    let mut msg = format!("GET:{}:", data_id);
    match data_id.as_str() {
      "TOURNAMENT_STATUS" => {
        let tournament = self.tournament.read().await.clone();
        if tournament.is_some() {
          msg += tournament.as_ref().unwrap().read().await.get_type().as_str();
        } else {
          msg += "false";
        }
      }
      "TOURNAMENT_DATA" => {
        let tournament = self.tournament.read().await.clone();
        if tournament.is_some() {
          let tourney_guard = tournament.as_ref().unwrap().read().await;
          let data = tourney_guard.get_data();
          if let Some(data) = data {
            msg += &data;
          }
        }
      }
      "BRACKET_PAIRINGS" => {
        let file = std::fs::read_to_string("bracket_pairings.txt").unwrap_or_default();
        msg += file.replace("\n", ",").as_str();
      }
      "MOVE_WAIT" => {
        let wait_time = *self.waiting_timeout.read().await as f64 / 1000f64;
        msg += wait_time.to_string().as_str();
      }
      "DEMO_MODE" => {
        let demo_mode = *self.demo_mode.read().await;
        msg += demo_mode.to_string().as_str();
      }
      "MAX_TIMEOUT" => {
        let max_time = *self.max_timeout.read().await as f64 / 1000f64;
        msg += max_time.to_string().as_str();
      }
      &_ => return Err(anyhow::anyhow!("ERROR:INVALID:GET")),
    }

    let _ = send(&tx, &msg);
    Ok(())
  }

  pub async fn handle_set_data(
    &self,
    tx: UnboundedSender<Message>,
    addr: SocketAddr,
    data_id: String,
    data_value: String,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    match data_id.as_str() {
      "DEMO_MODE" => {
        let demo_mode = data_value.parse::<bool>();
        if demo_mode.is_err() {
          return Err(anyhow::anyhow!("ERROR:INVALID:SET"));
        }
        *self.demo_mode.write().await = demo_mode.unwrap();
      }
      "MOVE_WAIT" => {
        let wait_time = data_value.parse::<f64>();
        if wait_time.is_err() {
          return Err(anyhow::anyhow!("ERROR:INVALID:SET"));
        }
        *self.waiting_timeout.write().await = (wait_time.unwrap() * 1000.0) as u64;
      }
      "MAX_TIMEOUT" => {
        let max_time = data_value.parse::<f64>();
        if max_time.is_err() {
          return Err(anyhow::anyhow!("ERROR:INVALID:SET"));
        }
        *self.max_timeout.write().await = (max_time.unwrap() * 1000.0) as u64;
      }
      "BRACKET_PAIRINGS" => {
        let file = data_value.replace(",", "\n");
        let result = std::fs::write("bracket_pairings.txt", file);
        match result {
          Ok(_) => (),
          Err(_) => return Err(anyhow::anyhow!("ERROR:INVALID:SET")),
        }
      }
      &_ => return Err(anyhow::anyhow!("ERROR:INVALID:SET")),
    }

    let _ = send(&tx, &format!("SET:{}:ACK", data_id));
    Ok(())
  }

  pub async fn handle_reservation_add(
    &self,
    tx: UnboundedSender<Message>,
    addr: SocketAddr,
    player1_username: String,
    player2_username: String,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    self.reservations.write().await.push((player1_username.clone(), player2_username.clone()));

    let _ = send(
      &tx,
      &format!("RESERVATION:ADD:{},{}", player1_username, player2_username),
    );

    let clients_guard = self.clients.read().await;
    let player1 = clients_guard.get(&player1_username).cloned();
    let player2 = clients_guard.get(&player2_username).cloned();

    if player1.is_some() && player2.is_some() {
      let player1 = player1.unwrap();
      let player2 = player2.unwrap();
      let mut player1 = player1.write().await;
      let mut player2 = player2.write().await;

      if player1.ready && player2.ready {
        let match_id: u32 = gen_match_id(&self.matches).await;
        let new_match = Arc::new(RwLock::new(Match::new(
          match_id,
          player1_username.clone(),
          player2_username.clone(),
          false,
        )));
        self.matches.write().await.insert(match_id, new_match.clone());
        let match_guard = new_match.read().await;
        self
          .broadcast(&format!(
            "GAME:START:{},{},{}",
            match_id, match_guard.player1, match_guard.player2
          ))
          .await;
        drop(match_guard);

        player1.ready = false;
        let player1_name = player1.username.clone();
        player1.current_match = Some(match_id);
        player1.color = if new_match.read().await.player1 == player1_username {
          let _ = send(&tx, "GAME:START:1");
          let _ = send(&player2.connection, "GAME:START:0");
          Color::Red
        } else {
          let _ = send(&tx, "GAME:START:0");
          let _ = send(&player2.connection, "GAME:START:1");
          Color::Yellow
        };

        player2.ready = false;
        let player2_name = player2.username.clone();
        player2.current_match = Some(match_id);
        player2.color = !player1.color;

        self
          .reservations
          .write()
          .await
          .retain(|(p1, p2)| !(p1 == &player1_username && p2 == &player2_username));

        drop(player2);
        drop(player1);
        drop(clients_guard);
        self.broadcast(&format!("READY:{}:{}", player1_name, false)).await;
        self.broadcast(&format!("READY:{}:{}", player2_name, false)).await;
      }
    }

    Ok(())
  }

  pub async fn handle_reservation_delete(
    &self,
    tx: UnboundedSender<Message>,
    addr: SocketAddr,
    player1_username: String,
    player2_username: String,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    self
      .reservations
      .write()
      .await
      .retain(|(p1, p2)| !(p1 == &player1_username && p2 == &player2_username));

    let _ = send(
      &tx,
      &format!(
        "RESERVATION:DELETE:{},{}",
        player1_username, player2_username
      ),
    );

    Ok(())
  }

  pub async fn handle_reservation_get(
    &self,
    tx: UnboundedSender<Message>,
    addr: SocketAddr,
  ) -> Result<(), anyhow::Error> {
    if !self.auth_check(addr).await {
      return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
    }

    let reservations_guard = self.reservations.read().await;
    let mut msg = "RESERVATION:LIST:".to_string();
    for (p1, p2) in reservations_guard.iter() {
      msg += &format!("{},{}|", p1, p2);
    }
    if msg.ends_with("|") {
      msg.pop();
    }

    let _ = send(&tx, &msg);
    Ok(())
  }

  pub async fn terminate_match(&self, match_id: u32) {
    let matches_guard = self.matches.read().await;
    let the_match = matches_guard.get(&match_id);
    if the_match.is_none() {
      error!(
        "Tried to call terminate_match on invalid matchID: {}",
        match_id
      );
    }
    let the_match = the_match.unwrap().read().await;

    if let Some(wait_thread) = &the_match.wait_thread {
      wait_thread.abort();
    }

    if let Some(timeout_thread) = &the_match.timeout_thread {
      timeout_thread.abort();
    }

    self.broadcast(&format!("GAME:{}:TERMINATED", the_match.id)).await;

    let clients_guard = self.clients.read().await;

    if the_match.player1 != SERVER_PLAYER_USERNAME.to_string() {
      let player1 = clients_guard.get(&the_match.player1).cloned();
      if let Some(player1) = player1 {
        let mut player1 = player1.write().await;
        let _ = send(&player1.connection, "GAME:TERMINATED");
        player1.current_match = None;
        player1.color = Color::None;
      }
    }

    if the_match.player2 != SERVER_PLAYER_USERNAME.to_string() {
      let player2 = clients_guard.get(&the_match.player2).cloned();
      if let Some(player2) = player2 {
        let mut player2 = player2.write().await;
        let _ = send(&player2.connection, "GAME:TERMINATED");
        player2.current_match = None;
        player2.color = Color::None;
      }
    }
    drop(clients_guard);

    drop(the_match);
    drop(matches_guard);

    self.matches.write().await.remove(&match_id);
  }

  pub async fn broadcast(&self, msg: &str) {
    let observers_guard = self.observers.read().await;
    for tx in observers_guard.values() {
      let _ = send(tx, msg);
    }
  }

  pub async fn auth_check(&self, addr: SocketAddr) -> bool {
    if self.admin.read().await.is_none() || self.admin.read().await.unwrap() != addr {
      return false;
    }
    true
  }

  pub async fn find_reservation_opponent(&self, username: &String) -> Option<String> {
    let reservations_guard = self.reservations.read().await;
    for (player1, player2) in reservations_guard.iter() {
      if player1 == username || player2 == username {
        let opponent_username = if player1 == username {
          player2
        } else {
          player1
        };

        let usernames_guard = self.usernames.read().await;
        return usernames_guard.iter().find_map(|(_, client_username)| {
          if client_username == opponent_username {
            Some(opponent_username.clone())
          } else {
            None
          }
        });
      }
    }

    None
  }
}
