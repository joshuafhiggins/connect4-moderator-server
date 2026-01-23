use std::time::Instant;

use crate::{tournaments::*, types::*, *};

pub struct Server {
    pub clients: Clients,
    pub usernames: Usernames,
    pub observers: Observers,
    pub matches: Matches,
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
            usernames: Arc::new(RwLock::new(HashMap::new())),
            observers: Arc::new(RwLock::new(HashMap::new())),
            matches: Arc::new(RwLock::new(HashMap::new())),
            admin: Arc::new(RwLock::new(None)),
            admin_password: Arc::new(admin_password),
            tournament: Arc::new(RwLock::new(None)),
            waiting_timeout: Arc::new(RwLock::new(5000)),
            max_timeout: Arc::new(RwLock::new(30000)),
            demo_mode: Arc::new(RwLock::new(demo_mode)),
        }
    }

    // Handler for CONNECT:<username>
    pub async fn handle_connect_cmd(
        &self,
        addr: SocketAddr,
        tx: UnboundedSender<Message>,
        requested_username: String,
    ) -> Result<(), anyhow::Error> {
        if requested_username.is_empty() {
            return Err(anyhow::anyhow!(format!(
                "ERROR:INVALID:ID:{}",
                requested_username
            )));
        }

        if requested_username == SERVER_PLAYER_USERNAME {
            return Err(anyhow::anyhow!(format!(
                "ERROR:INVALID:ID:{}",
                requested_username
            )));
        }

        let clients_guard = self.clients.read().await;
        for client in clients_guard.values() {
            if requested_username == client.read().await.username {
                return Err(anyhow::anyhow!(format!(
                    "ERROR:INVALID:ID:{}",
                    requested_username
                )));
            }
        }

        drop(clients_guard);

        self.remove_observer_from_all_matches(addr).await;

        // not taken
        self.observers.write().await.remove(&addr);
        self.usernames.write().await.insert(requested_username.clone(), addr);
        self.clients.write().await.insert(
            addr.to_string().parse()?,
            Arc::new(RwLock::new(Client::new(
                requested_username,
                tx.clone(),
                addr.to_string().parse()?,
            ))),
        );

        let _ = send(&tx, "CONNECT:ACK");
        Ok(())
    }

    pub async fn handle_disconnect_cmd(
        &self,
        addr: SocketAddr,
        tx: UnboundedSender<Message>,
    ) -> Result<(), anyhow::Error> {
        let clients_guard = self.clients.read().await;
        let client_opt = clients_guard.get(&addr).cloned();

        if client_opt.is_none() {
            return Err(anyhow::anyhow!("ERROR:INVALID:DISCONNECT"));
        }

        drop(clients_guard);

        let mut client = client_opt.as_ref().unwrap().write().await;
        self.usernames.write().await.remove(&client.username);
        client.ready = false;
        client.color = Color::None;

        if client.current_match.is_some() {
            let match_id = client.current_match.unwrap();
            drop(client);

            self.terminate_match(match_id).await;
        }

        self.clients.write().await.remove(&addr);
        self.observers.write().await.insert(addr, tx.clone());
        let _ = send(&tx, "DISCONNECT:ACK");

        Ok(())
    }

    // Handler for READY
    pub async fn handle_ready(
        &self,
        addr: SocketAddr,
        tx: UnboundedSender<Message>,
    ) -> Result<(), anyhow::Error> {
        let clients_guard = self.clients.read().await;
        if clients_guard.get(&addr).is_none() {
            return Err(anyhow::anyhow!("ERROR:INVALID"));
        }

        if clients_guard.get(&addr).unwrap().read().await.ready {
            return Err(anyhow::anyhow!("ERROR:INVALID:READY"));
        }

        let mut client = clients_guard.get(&addr).unwrap().write().await;
        client.ready = true;
        let _ = send(&tx, "READY:ACK");

        let is_demo_mode = self.demo_mode.read().await.clone();
        if is_demo_mode {
            let match_id: u32 = gen_match_id(&self.matches).await;
            let new_match = Arc::new(RwLock::new(Match::new(
                match_id,
                addr.to_string().parse()?,
                SERVER_PLAYER_ADDR.to_string().parse()?,
                is_demo_mode,
            )));
            self.matches.write().await.insert(match_id, new_match.clone());
            client.ready = false;
            client.current_match = Some(match_id);
            client.color = if new_match.read().await.player1 == addr {
                let _ = send(&tx, "GAME:START:1");
                Color::Red
            } else {
                let _ = send(&tx, "GAME:START:0");
                Color::Yellow
            };
        }

        Ok(())
    }

    // Handler for PLAY (column already parsed)
    pub async fn handle_play(
        &self,
        addr: SocketAddr,
        tx: UnboundedSender<Message>,
        column: usize,
    ) -> Result<(), anyhow::Error> {
        let clients_guard = self.clients.read().await;
        let client_opt = clients_guard.get(&addr);

        // Check if client is valid
        if client_opt.is_none() || client_opt.unwrap().read().await.current_match.is_none() {
            return Err(anyhow::anyhow!("ERROR:INVALID:MOVE"));
        }
        let client = client_opt.unwrap().read().await;

        let matches_guard = self.matches.read().await;
        let current_match = matches_guard.get(&client.current_match.unwrap()).unwrap().read().await;

        let opponent_addr = if addr == current_match.player1 {
            current_match.player2
        } else {
            current_match.player1
        };

        let opponent_connection = if current_match.demo_mode {
            None
        } else if addr == current_match.player1 {
            Some(clients_guard.get(&current_match.player2).unwrap().read().await.connection.clone())
        } else {
            Some(clients_guard.get(&current_match.player1).unwrap().read().await.connection.clone())
        };

        let opponent_username = if current_match.demo_mode {
            SERVER_PLAYER_USERNAME.to_string()
        } else if addr == current_match.player1 {
            clients_guard.get(&current_match.player2).unwrap().read().await.username.clone()
        } else {
            clients_guard.get(&current_match.player1).unwrap().read().await.username.clone()
        };

        // Check if it's their move
        let mut invalid = false;
        if (current_match.ledger.is_empty() && current_match.player1 != addr)
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

        // Terminate games if a player makes an invalid move
        if invalid {
            let current_match_id = current_match.id;
            let is_demo_mode = current_match.demo_mode;
            let viewers = current_match.viewers.clone();

            drop(current_match);
            drop(matches_guard);
            drop(client);
            drop(clients_guard);

            if is_demo_mode {
                self.terminate_match(current_match_id).await;
                tx.send(Message::Close(None))?;
            } else {
                let _ = send(&tx, "GAME:LOSS");
                let _ = send(&opponent_connection.unwrap(), "GAME:WINS");
                self.broadcast_message(&viewers, &format!("GAME:WIN:{}", opponent_username)).await;

                let mut clients_guard = self.clients.write().await;
                let mut client = clients_guard.get_mut(&addr).unwrap().write().await;
                client.current_match = None;
                client.color = Color::None;
                drop(client);

                let mut opponent = clients_guard.get_mut(&opponent_addr).unwrap().write().await;
                opponent.current_match = None;
                opponent.color = Color::None;
                drop(opponent);

                let mut tournament_guard = self.tournament.write().await;
                let tourney = tournament_guard.as_mut().unwrap();
                tourney.write().await.inform_winnder(opponent_addr, false);
                drop(tournament_guard);

                self.matches.write().await.remove(&current_match_id).unwrap();
            }
            return Ok(());
        }

        current_match.place_token(client.color.clone(), column);

        if let Some(timeout_thread) = &current_match.timeout_thread {
            timeout_thread.abort();
        }

        let mut viewer_messages = Vec::new();
        let viewers = current_match.viewers.clone();

        viewer_messages.push(format!("GAME:MOVE:{}:{}", client.username, column));

        // Check game end conditions
        let (winner, filled) = current_match.end_game_check();

        // Send match results
        if winner == client.color {
            let _ = send(&tx, "GAME:WINS");
            if !current_match.demo_mode {
                let _ = send(&opponent_connection.as_ref().unwrap(), "GAME:LOSS");
            }
            viewer_messages.push(format!("GAME:WIN:{}", client.username));
        } else if filled {
            let _ = send(&tx, "GAME:DRAW");
            if !current_match.demo_mode {
                let _ = send(&opponent_connection.as_ref().unwrap(), "GAME:DRAW");
            }
            viewer_messages.push("GAME:DRAW".to_string());
        }

        // remove match from matchmaker
        if winner != Color::None || filled {
            let current_match_id = current_match.id;
            let is_demo_mode = current_match.demo_mode;

            drop(client);
            drop(current_match);
            drop(clients_guard);

            let clients_guard = self.clients.read().await;
            let mut client = clients_guard.get(&addr).unwrap().write().await;
            client.current_match = None;
            client.color = Color::None;
            drop(client);

            if !is_demo_mode {
                let mut opponent = clients_guard.get(&opponent_addr).unwrap().write().await;
                opponent.current_match = None;
                opponent.color = Color::None;
                drop(opponent);
            }

            matches_guard.remove(&current_match_id).unwrap();

            if self.tournament.read().await.is_some() && matches_guard.is_empty() {
                drop(matches_guard);
                drop(clients_guard);

                let mut tournament_guard = self.tournament.write().await;
                let tourney = tournament_guard.as_mut().unwrap();
                tourney.write().await.inform_winnder(addr, filled);
                tourney.write().await.next(&self).await;
                if tourney.read().await.is_completed() {
                    *tournament_guard = None;
                }
            } else if self.tournament.read().await.is_none() {
                let _ = send(&tx, "TOURNAMENT:END");
                if !is_demo_mode {
                    let _ = send(&opponent_connection.unwrap(), "TOURNAMENT:END");
                }
            }

            return Ok(());
        }

        let default_waiting_time = *self.waiting_timeout.read().await;
        let mut adjusted_waiting =
            default_waiting_time as i64 + (rand::rng().random_range(0..=50) - 25);
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
        let opp_connection_move = opponent_connection.clone();
        let client_tx = tx.clone();
        if current_match.demo_mode {
            current_match.ledger.push((!client.color, demo_move, Instant::now()));
            current_match.place_token(!client.color, demo_move);
        }

        current_match.wait_thread = Some(tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(adjusted_waiting as u64)).await;

            if !demo_mode && no_winner {
                let _ = send(
                    &opp_connection_move.as_ref().unwrap(),
                    &format!("OPPONENT:{}", column),
                );
            }

            for msg in viewer_messages {
                broadcast_message(&observers, &viewers, &msg).await;
            }

            if demo_mode && no_winner {
                tokio::time::sleep(tokio::time::Duration::from_millis(default_waiting_time)).await;
                let _ = send(&client_tx, &format!("OPPONENT:{}", demo_move));
                broadcast_message(
                    &observers,
                    &viewers,
                    &format!("GAME:MOVE:{}:{}", SERVER_PLAYER_USERNAME, demo_move),
                )
                .await;
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
        let client_addr = addr.clone();
        let observers = self.observers.clone();
        let viewers = current_match.viewers.clone();
        current_match.timeout_thread = Some(tokio::spawn(async move {
            if demo_mode {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(adjusted_waiting as u64)).await;
            tokio::time::sleep(tokio::time::Duration::from_millis(max_timeout as u64)).await;

            let matches_guard = matches.read().await;
            let the_match = matches_guard.get(&match_id);
            if let Some(the_match) = the_match {
                let the_match = the_match.read().await;
                if the_match.ledger.len() == ledger_size {
                    // forfeit the match
                    let _ = send(&client_tx, "GAME:WINS");
                    let _ = send(&opponent_connection.unwrap(), "GAME:LOSS");
                    broadcast_message(
                        &observers,
                        &viewers,
                        &format!("GAME:WIN:{}", client_username),
                    )
                    .await;

                    let mut clients_guard = clients.write().await;
                    let mut client = clients_guard.get_mut(&client_addr).unwrap().write().await;
                    client.current_match = None;
                    client.color = Color::None;
                    drop(client);

                    let mut opponent = clients_guard.get_mut(&opponent_addr).unwrap().write().await;
                    opponent.current_match = None;
                    opponent.color = Color::None;
                    drop(opponent);

                    let mut tournament_guard = tournament.write().await;
                    let tourney = tournament_guard.as_mut().unwrap();
                    tourney.write().await.inform_winnder(client_addr, false);
                    drop(tournament_guard);

                    matches.write().await.remove(&match_id).unwrap();
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

    pub async fn handle_game_list(
        &self,
        tx: UnboundedSender<Message>,
    ) -> Result<(), anyhow::Error> {
        let matches_guard = self.matches.read().await;
        let clients_guard = self.clients.read().await;
        let mut to_send = "GAME:LIST:".to_string();
        for match_guard in matches_guard.values() {
            let a_match = match_guard.read().await;
            let player1 = if a_match.player1.to_string() == SERVER_PLAYER_ADDR {
                SERVER_PLAYER_USERNAME.to_string()
            } else {
                clients_guard.get(&a_match.player1).unwrap().read().await.username.clone()
            };
            let player2 = if a_match.player2.to_string() == SERVER_PLAYER_ADDR {
                SERVER_PLAYER_USERNAME.to_string()
            } else {
                clients_guard.get(&a_match.player2).unwrap().read().await.username.clone()
            };
            to_send += a_match.id.to_string().as_str();
            to_send += ",";
            to_send += player1.as_str();
            to_send += ",";
            to_send += player2.as_str();
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
        addr: SocketAddr,
    ) -> Result<(), anyhow::Error> {
        let result = self.watch(match_id, addr).await;
        if result.is_err() {
            return Err(anyhow::anyhow!("ERROR:INVALID:WATCH"));
        }

        let clients_guard = self.clients.read().await;
        let matches_guard = self.matches.read().await;
        let the_match = matches_guard.get(&match_id).unwrap().read().await;

        let player1 = if !the_match.player1.to_string().eq(SERVER_PLAYER_ADDR) {
            clients_guard.get(&the_match.player1).unwrap().read().await.username.clone()
        } else {
            SERVER_PLAYER_USERNAME.to_string()
        };

        let player2 = if !the_match.player2.to_string().eq(SERVER_PLAYER_ADDR) {
            clients_guard.get(&the_match.player2).unwrap().read().await.username.clone()
        } else {
            SERVER_PLAYER_USERNAME.to_string()
        };

        let ledger = the_match.ledger.clone();

        drop(clients_guard);
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

        let usernames_guard = self.usernames.read().await;
        let clients_guard = self.clients.read().await;

        let kick_addr_result = usernames_guard.get(&kick_username);
        match kick_addr_result {
            Some(kick_addr) => {
                let kick_client = clients_guard.get(kick_addr).unwrap().read().await;
                kick_client.connection.send(Message::Close(None))?;
            }
            None => return Err(anyhow::anyhow!("ERROR:INVALID:KICK")),
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
                *tournament_guard = None;
            }
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
        for (client_addr, client_guard) in clients_guard.iter_mut() {
            if client_guard.read().await.ready {
                ready_players.push(*client_addr);
            }
        }

        if ready_players.len() < 3 {
            return Err(anyhow::anyhow!("ERROR:INVALID:TOURNAMENT"));
        }

        drop(clients_guard);

        let mut tourney = match tournament_type.as_str() {
            "RoundRobin" => RoundRobin::new(&ready_players),
            &_ => RoundRobin::new(&ready_players),
        };
        tourney.start(&self).await;

        let mut tournament_guard = self.tournament.write().await;
        *tournament_guard = Some(Arc::new(RwLock::new(tourney)));

        self.broadcast_message_all_observers(&format!("TOURNAMENT:START:{}", tournament_type))
            .await;
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

        self.broadcast_message_all_observers("TOURNAMENT:CANCEL").await;
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
            &_ => return Err(anyhow::anyhow!("ERROR:INVALID:SET")),
        }

        let _ = send(&tx, &format!("SET:{}:ACK", data_id));
        Ok(())
    }

    pub async fn watch(&self, new_match_id: u32, addr: SocketAddr) -> Result<(), String> {
        let matches_guard = self.matches.read().await;

        for match_guard in matches_guard.values() {
            let mut found = false;
            let mut a_match = match_guard.write().await;
            for i in 0..a_match.viewers.len() {
                if a_match.viewers[i] == addr {
                    a_match.viewers.remove(i);
                    found = true;
                    break;
                }
            }

            if found {
                break;
            }
        }

        let result = matches_guard.get(&new_match_id);
        if result.is_none() {
            return Err("Match not found".to_string());
        }
        result.unwrap().write().await.viewers.push(addr);

        Ok(())
    }

    pub async fn remove_observer_from_all_matches(&self, addr: SocketAddr) {
        let matches_guard = self.matches.read().await;

        for match_guard in matches_guard.values() {
            let mut found = false;
            let mut a_match = match_guard.write().await;
            for i in 0..a_match.viewers.len() {
                if a_match.viewers[i] == addr {
                    a_match.viewers.remove(i);
                    found = true;
                    break;
                }
            }

            if found {
                break;
            }
        }
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

        self.broadcast_message(&the_match.viewers, "GAME:TERMINATED").await;

        let clients_guard = self.clients.read().await;
        if the_match.player1 != SERVER_PLAYER_ADDR.to_string().parse().unwrap() {
            let mut player1 = clients_guard.get(&the_match.player1).unwrap().write().await;
            let _ = send(&player1.connection, "GAME:TERMINATED");
            player1.current_match = None;
            player1.color = Color::None;
        }

        if the_match.player2 != SERVER_PLAYER_ADDR.to_string().parse().unwrap() {
            let mut player2 = clients_guard.get(&the_match.player2).unwrap().write().await;
            let _ = send(&player2.connection, "GAME:TERMINATED");
            player2.current_match = None;
            player2.color = Color::None;
        }
        drop(clients_guard);

        drop(the_match);
        drop(matches_guard);

        self.matches.write().await.remove(&match_id);
    }

    pub async fn broadcast_message(&self, addrs: &Vec<SocketAddr>, msg: &str) {
        for addr in addrs {
            let observers_guard = self.observers.read().await;
            let tx = observers_guard.get(addr);
            if tx.is_none() {
                continue;
            }
            let _ = send(tx.unwrap(), msg);
        }
    }

    pub async fn broadcast_message_all_observers(&self, msg: &str) {
        let observers_guard = self.observers.read().await;
        for (_, tx) in observers_guard.iter() {
            let _ = send(tx, msg);
        }
    }

    pub async fn auth_check(&self, addr: SocketAddr) -> bool {
        if self.admin.read().await.is_none() || self.admin.read().await.unwrap() != addr {
            return false;
        }
        true
    }
}
