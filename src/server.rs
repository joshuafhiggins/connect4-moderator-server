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

        let mut reconnecting = false;
        let disconnected_guard = self.disconnected_clients.read().await;
        if disconnected_guard.contains(&requested_username) {
            reconnecting = true;
        }

        let clients_guard = self.clients.read().await;
        let mut reconnecting_client = None;
        for client in clients_guard.values() {
            if requested_username == client.read().await.username {
                if reconnecting {
                    reconnecting_client = Some(client.clone());
                    break;
                }

                return Err(anyhow::anyhow!(format!(
                    "ERROR:INVALID:ID:{}",
                    requested_username
                )));
            }
        }

        drop(clients_guard);

        self.remove_observer_from_all_matches(addr).await;
        self.observers.write().await.remove(&addr);
        self.usernames.write().await.insert(requested_username.clone(), addr);
        let _ = send(&tx, "CONNECT:ACK");

        if !reconnecting {
            self.clients.write().await.insert(
                addr.to_string().parse()?,
                Arc::new(RwLock::new(Client::new(
                    requested_username,
                    tx.clone(),
                    addr.to_string().parse()?,
                ))),
            );

            return Ok(());
        }

        // reconnecting
        self.disconnected_clients.write().await.retain(|name| name != &requested_username);
        let client_guard = reconnecting_client.unwrap();
        let mut client = client_guard.write().await;
        let old_addr = client.addr;
        client.addr = addr;
        client.connection = tx.clone();
        // I don't think this will fail
        let match_id = client.current_match.unwrap();
        let client_color = client.color;

        drop(client);

        let mut clients_guard = self.clients.write().await;
        clients_guard.remove(&old_addr);
        clients_guard.insert(addr, client_guard.clone());
        drop(clients_guard);

        let tournament_guard = self.tournament.read().await;
        if tournament_guard.is_some() {
            let tourney = tournament_guard.clone().unwrap();
            tourney.write().await.inform_reconnect(old_addr, addr);
        }
        drop(tournament_guard);

        let matches_guard = self.matches.read().await;
        let mut the_match = matches_guard.get(&match_id).unwrap().write().await;
        if the_match.demo_mode {
            drop(the_match);
            drop(matches_guard);
            self.terminate_match(match_id).await;
            return Ok(());
        } else {
            the_match.ledger.clear();
            the_match.board = vec![vec![Color::None; 6]; 7];
            let opponent_addr = if the_match.player1 == addr {
                the_match.player2
            } else {
                the_match.player1
            };

            if the_match.wait_thread.is_some() {
                the_match.wait_thread.as_ref().unwrap().abort();
            }

            if the_match.timeout_thread.is_some() {
                the_match.timeout_thread.as_ref().unwrap().abort();
            }

            let clients_guard = self.clients.read().await;
            let opponent = clients_guard.get(&opponent_addr).unwrap().read().await;
            let _ = send(&opponent.connection, "GAME:TERMINATED");
            let _ = send(
                &tx,
                &format!("GAME:START:{}", bool::from(client_color) as u8),
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
        let mut found_client = None;

        for client in clients_guard.values() {
            if requested_username == client.read().await.username {
                if disconnected_guard.contains(&requested_username) {
                    found_client = Some(client.clone());
                }
                break;
            }
        }

        drop(clients_guard);
        drop(disconnected_guard);

        if let Some(client_guard) = found_client {
            self.disconnected_clients.write().await.retain(|name| name != &requested_username);
            let mut client = client_guard.write().await;
            let old_addr = client.addr;
            client.addr = addr;
            client.connection = tx.clone();

            let mut clients_guard = self.clients.write().await;
            clients_guard.remove(&old_addr);
            clients_guard.insert(addr, client_guard.clone());
            drop(clients_guard);

            let _ = send(&tx, "RECONNECT:ACK");

            let matches_guard = self.matches.read().await;
            let the_match = matches_guard.get(&client.current_match.unwrap()).unwrap().read().await;

            let last = the_match.ledger.last();
            if last.is_some() && last.unwrap().0 != client.color {
                let _ = send(
                    &tx,
                    &format!("OPPONENT:{}", the_match.ledger.last().unwrap().1),
                );
            }
        } else {
            return Err(anyhow::anyhow!(format!(
                "ERROR:INVALID:RECONNECT:{}",
                requested_username
            )));
        }

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
        let client_username = client.username.clone();
        client.ready = true;
        let _ = send(&tx, "READY:ACK");
        drop(client);
        drop(clients_guard);

        if let Some(opponent_addr) = self.find_reservation_opponent(client_username).await {
            let clients_guard = self.clients.read().await;
            let mut client = clients_guard.get(&addr).unwrap().write().await;
            let mut opponent = clients_guard.get(&opponent_addr).unwrap().write().await;

            let match_id: u32 = gen_match_id(&self.matches).await;
            let new_match = Arc::new(RwLock::new(Match::new(
                match_id,
                addr,
                opponent_addr,
                false,
            )));
            self.matches.write().await.insert(match_id, new_match.clone());

            client.ready = false;
            client.current_match = Some(match_id);
            client.color = if new_match.read().await.player1 == addr {
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

            self.reservations
                .write()
                .await
                .retain(|(p1, p2)| !(p1 == &client.username && p2 == &opponent.username));

            return Ok(());
        }

        let clients_guard = self.clients.read().await;
        let mut client = clients_guard.get(&addr).unwrap().write().await;
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

        let opponent = {
            let mut result = None;

            if !current_match.demo_mode {
                let opponent_addr = if addr == current_match.player1 {
                    current_match.player2
                } else {
                    current_match.player1
                };

                result = Some(clients_guard.get(&opponent_addr).cloned().unwrap());
            }

            result
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
                let opponent = opponent.unwrap();
                let mut opponent = opponent.write().await;

                let _ = send(&tx, "GAME:LOSS");
                let _ = send(&opponent.connection, "GAME:WINS");
                self.broadcast_message(&viewers, &format!("GAME:WIN:{}", opponent.username)).await;

                opponent.current_match = None;
                opponent.color = Color::None;
                let opponent_addr = opponent.addr;
                drop(opponent);

                let mut clients_guard = self.clients.write().await;
                let mut client = clients_guard.get_mut(&addr).unwrap().write().await;
                client.current_match = None;
                client.color = Color::None;
                drop(client);

                let mut tournament_guard = self.tournament.write().await;
                let tourney = tournament_guard.as_mut().unwrap();
                tourney.write().await.inform_winner(opponent_addr, false);
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
                let opponent = opponent.clone().unwrap();
                let opponent = opponent.read().await;
                let _ = send(&opponent.connection, "GAME:LOSS");
            }
            viewer_messages.push(format!("GAME:WIN:{}", client.username));
        } else if filled {
            let _ = send(&tx, "GAME:DRAW");
            if !current_match.demo_mode {
                let opponent = opponent.clone().unwrap();
                let opponent = opponent.read().await;
                let _ = send(&opponent.connection, "GAME:DRAW");
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
                let opponent = opponent.clone().unwrap();
                let mut opponent = opponent.write().await;
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
                tourney.write().await.inform_winner(addr, filled);
                tourney.write().await.next(&self).await;
                if tourney.read().await.is_completed() {
                    *tournament_guard = None;
                }
            } else if self.tournament.read().await.is_none() {
                let _ = send(&tx, "TOURNAMENT:END");
                if !is_demo_mode {
                    let opponent = opponent.clone().unwrap();
                    let opponent = opponent.read().await;
                    let _ = send(&opponent.connection, "TOURNAMENT:END");
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
        let opponent_move = opponent.clone();
        let client_tx = tx.clone();
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
        let opponent_move = opponent.clone();
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
                    let opponent = opponent_move.clone().unwrap();
                    let opponent = opponent.read().await;
                    let _ = send(&opponent.connection, "GAME:LOSS");
                    drop(opponent);
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

                    let opponent = opponent_move.unwrap();
                    let mut opponent = opponent.write().await;
                    opponent.current_match = None;
                    opponent.color = Color::None;
                    drop(opponent);

                    let mut tournament_guard = tournament.write().await;
                    let tourney = tournament_guard.as_mut().unwrap();
                    tourney.write().await.inform_winner(client_addr, false);
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

    pub async fn handle_game_award(
        &self,
        addr: SocketAddr,
        match_id: u32,
        winner_username: String,
    ) -> Result<(), anyhow::Error> {
        if !self.auth_check(addr).await {
            return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
        }

        let server_player_addr: SocketAddr = SERVER_PLAYER_ADDR.to_string().parse()?;

        let (player1_addr, player2_addr, viewers, demo_mode) = {
            let mut matches_guard = self.matches.write().await;
            let the_match = matches_guard
                .get(&match_id)
                .ok_or_else(|| anyhow::anyhow!("ERROR:INVALID:AWARD"))?
                .clone();
            let mut the_match = the_match.write().await;

            if let Some(wait_thread) = &the_match.wait_thread {
                wait_thread.abort();
            }

            if let Some(timeout_thread) = &the_match.timeout_thread {
                timeout_thread.abort();
            }

            let player1_addr = the_match.player1;
            let player2_addr = the_match.player2;
            let viewers = the_match.viewers.clone();
            let demo_mode = the_match.demo_mode;

            matches_guard.remove(&match_id);

            (player1_addr, player2_addr, viewers, demo_mode)
        };

        let clients_guard = self.clients.read().await;
        let player1_name = if player1_addr == server_player_addr {
            SERVER_PLAYER_USERNAME.to_string()
        } else {
            clients_guard
                .get(&player1_addr)
                .ok_or_else(|| anyhow::anyhow!("ERROR:INVALID:AWARD"))?
                .read()
                .await
                .username
                .clone()
        };
        let player2_name = if player2_addr == server_player_addr {
            SERVER_PLAYER_USERNAME.to_string()
        } else {
            clients_guard
                .get(&player2_addr)
                .ok_or_else(|| anyhow::anyhow!("ERROR:INVALID:AWARD"))?
                .read()
                .await
                .username
                .clone()
        };
        drop(clients_guard);

        let winner_username = winner_username.trim().to_string();
        let winner_is_player1 = winner_username == player1_name;
        let winner_is_player2 = winner_username == player2_name;

        if !winner_is_player1 && !winner_is_player2 {
            return Err(anyhow::anyhow!("ERROR:INVALID:AWARD"));
        }

        let winner_addr = if winner_is_player1 {
            player1_addr
        } else {
            player2_addr
        };
        let loser_addr = if winner_is_player1 {
            player2_addr
        } else {
            player1_addr
        };

        self.broadcast_message(&viewers, &format!("GAME:WIN:{}", winner_username))
            .await;

        let mut clients_guard = self.clients.write().await;
        if winner_addr != server_player_addr {
            let mut winner = clients_guard
                .get_mut(&winner_addr)
                .ok_or_else(|| anyhow::anyhow!("ERROR:INVALID:AWARD"))?
                .write()
                .await;
            let _ = send(&winner.connection, "GAME:WINS");
            winner.current_match = None;
            winner.color = Color::None;
        }

        if loser_addr != server_player_addr {
            let mut loser = clients_guard
                .get_mut(&loser_addr)
                .ok_or_else(|| anyhow::anyhow!("ERROR:INVALID:AWARD"))?
                .write()
                .await;
            let _ = send(&loser.connection, "GAME:LOSS");
            loser.current_match = None;
            loser.color = Color::None;
        }
        drop(clients_guard);

        if self.tournament.read().await.is_some() && self.matches.read().await.is_empty() {
            let mut tournament_guard = self.tournament.write().await;
            let tourney = tournament_guard.as_mut().unwrap();
            tourney.write().await.inform_winner(winner_addr, false);
            tourney.write().await.next(&self).await;
            if tourney.read().await.is_completed() {
                *tournament_guard = None;
            }
        } else if !demo_mode && self.tournament.read().await.is_none() {
            let clients_guard = self.clients.read().await;
            if winner_addr != server_player_addr {
                if let Some(winner) = clients_guard.get(&winner_addr) {
                    let _ = send(&winner.read().await.connection, "TOURNAMENT:END");
                }
            }
            if loser_addr != server_player_addr {
                if let Some(loser) = clients_guard.get(&loser_addr) {
                    let _ = send(&loser.read().await.connection, "TOURNAMENT:END");
                }
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

        // Clear any pending reservations when a tournament starts
        self.reservations.write().await.clear();

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

        let player1_addr = self.usernames.read().await.get(&player1_username).cloned();
        let player2_addr = self.usernames.read().await.get(&player2_username).cloned();

        let clients_guard = self.clients.read().await;
        if player1_addr.is_some() && player2_addr.is_some() {
            let mut player1 = clients_guard.get(&player1_addr.unwrap()).unwrap().write().await;
            let mut player2 = clients_guard.get(&player2_addr.unwrap()).unwrap().write().await;

            if player1.ready && player2.ready {
                let match_id: u32 = gen_match_id(&self.matches).await;
                let new_match = Arc::new(RwLock::new(Match::new(
                    match_id,
                    player1_addr.unwrap(),
                    player2_addr.unwrap(),
                    false,
                )));
                self.matches.write().await.insert(match_id, new_match.clone());

                player1.ready = false;
                player1.current_match = Some(match_id);
                player1.color = if new_match.read().await.player1 == player1_addr.unwrap() {
                    let _ = send(&tx, "GAME:START:1");
                    let _ = send(&player2.connection, "GAME:START:0");
                    Color::Red
                } else {
                    let _ = send(&tx, "GAME:START:0");
                    let _ = send(&player2.connection, "GAME:START:1");
                    Color::Yellow
                };

                player2.ready = false;
                player2.current_match = Some(match_id);
                player2.color = !player1.color;

                self.reservations
                    .write()
                    .await
                    .retain(|(p1, p2)| !(p1 == &player1_username && p2 == &player2_username));
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

        self.reservations
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

    pub async fn find_reservation_opponent(&self, username: String) -> Option<SocketAddr> {
        let reservations_guard = self.reservations.read().await;
        for (player1, player2) in reservations_guard.iter() {
            if player1 == &username || player2 == &username {
                let opponent_username = if player1 == &username {
                    player2
                } else {
                    player1
                };

                let usernames_guard = self.usernames.read().await;
                if let Some(opponent_addr) = usernames_guard.get(opponent_username) {
                    let clients_guard = self.clients.read().await;
                    let opponent = clients_guard.get(opponent_addr).unwrap().read().await;
                    if opponent.ready {
                        return Some(*opponent_addr);
                    }
                }
            }
        }

        None
    }
}
