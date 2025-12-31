use crate::*;
use rand::Rng;
use std::net::SocketAddr;
use std::sync::Arc;
use std::vec;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

pub struct Server {
	pub clients: Clients,
	pub usernames: Usernames,
	pub observers: Observers,
	pub matches: Matches,
	pub admin: Arc<RwLock<Option<SocketAddr>>>,
	pub admin_password: Arc<String>,
	pub tournament: WrappedTournament,
	pub waiting_timeout: Arc<RwLock<u64>>,
	pub demo_mode: bool,
	pub tournament_type: String,
}

impl Server {
	pub fn new(admin_password: String, demo_mode: bool, tournament_type: String) -> Server {
		Server {
			clients: Arc::new(RwLock::new(HashMap::new())),
			usernames: Arc::new(RwLock::new(HashMap::new())),
			observers: Arc::new(RwLock::new(HashMap::new())),
			matches: Arc::new(RwLock::new(HashMap::new())),
			admin: Arc::new(RwLock::new(None)),
			admin_password: Arc::new(admin_password),
			tournament: Arc::new(RwLock::new(None)),
			waiting_timeout: Arc::new(RwLock::new(5000)),
			demo_mode,
			tournament_type,
		}
	}

	// Handler for CONNECT:<username>
	pub async fn handle_connect(
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

		let _ = crate::send(&tx, "CONNECT:ACK");
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
			return Err(anyhow::anyhow!("ERROR:INVALID"));
		}

		let mut client = clients_guard.get(&addr).unwrap().write().await;
		client.ready = true;
		let _ = crate::send(&tx, "READY:ACK");

		if self.demo_mode {
			let match_id: u32 = crate::gen_match_id(&self.matches).await;
			let new_match = Arc::new(RwLock::new(Match::new(
				match_id,
				addr.to_string().parse()?,
				addr.to_string().parse()?,
			)));
			self.matches.write().await.insert(match_id, new_match.clone());
			client.ready = false;
			client.current_match = Some(match_id);
			client.color = Color::Red;
			let _ = crate::send(&tx, "GAME:START:1");
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

		let opponent_connection = if addr == current_match.player1 {
			clients_guard.get(&current_match.player2).unwrap().read().await.connection.clone()
		} else {
			clients_guard.get(&current_match.player1).unwrap().read().await.connection.clone()
		};

		let opponent_username = if addr == current_match.player1 {
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
			let _ = crate::send(&tx, "ERROR:INVALID:MOVE");
			invalid = true;
		}

		drop(current_match);
		drop(matches_guard);

		let mut matches_guard = self.matches.write().await;
		let mut current_match =
			matches_guard.get_mut(&client.current_match.unwrap()).unwrap().write().await;

		// Check if valid move
		if column >= 7 && !invalid {
			let _ = crate::send(&tx, "ERROR:INVALID:MOVE");
			invalid = true;
		}

		if current_match.board[column][5] != Color::None && !invalid {
			let _ = crate::send(&tx, "ERROR:INVALID:MOVE");
			invalid = true;
		}

		// Terminate games if a player makes an invalid move
		if invalid {
			let current_match_id = current_match.id;
			let viewers = current_match.viewers.clone();

			drop(current_match);
			drop(matches_guard);
			drop(client);
			drop(clients_guard);

			if self.demo_mode {
				crate::terminate_match(
					current_match_id,
					&self.matches,
					&self.clients,
					&self.observers,
					self.demo_mode,
				)
				.await;
				tx.send(Message::Close(None))?;
			} else {
				let _ = crate::send(&tx, "GAME:LOSS");
				let _ = crate::send(&opponent_connection, "GAME:WINS");
				crate::broadcast_message(
					&viewers,
					&self.observers,
					&format!("GAME:WIN:{}", opponent_username),
				)
				.await;

				let mut clients_guard = self.clients.write().await;
				let mut client = clients_guard.get_mut(&addr).unwrap().write().await;
				client.current_match = None;
				client.color = Color::None;
				drop(client);

				let mut opponent = clients_guard.get_mut(&opponent_addr).unwrap().write().await;
				opponent.current_match = None;
				opponent.color = Color::None;
				opponent.score += 1;
				drop(opponent);

				self.matches.write().await.remove(&current_match_id).unwrap();
			}
			return Ok(());
		} else {
			// Place it
			current_match.place_token(client.color.clone(), column);
			current_match.move_to_dispatch = (client.color.clone(), column);
		}

		// broadcast the move to viewers
		crate::broadcast_message(
			&current_match.viewers,
			&self.observers,
			&format!("GAME:MOVE:{}:{}", client.username, column),
		)
		.await;

		// Check game end conditions
		let (winner, filled) = crate::end_game_check(&current_match.board);

		if winner != Color::None {
			if winner == client.color {
				let _ = crate::send(&tx, "GAME:WINS");
				if !self.demo_mode {
					let _ = crate::send(&opponent_connection, "GAME:LOSS");
				}
				crate::broadcast_message(
					&current_match.viewers,
					&self.observers,
					&format!("GAME:WIN:{}", client.username),
				)
				.await;
			} else {
				let _ = crate::send(&tx, "GAME:LOSS");
				if !self.demo_mode {
					let _ = crate::send(&opponent_connection, "GAME:WINS");
				}
				crate::broadcast_message(
					&current_match.viewers,
					&self.observers,
					&format!("GAME:WIN:{}", opponent_username),
				)
				.await;
			}
		} else if filled {
			let _ = crate::send(&tx, "GAME:DRAW");
			if !self.demo_mode {
				let _ = crate::send(&opponent_connection, "GAME:DRAW");
			}
			crate::broadcast_message(&current_match.viewers, &self.observers, "GAME:DRAW").await;
		}

		// remove match from matchmaker
		if winner != Color::None || filled {
			let current_match_id = current_match.id;

			drop(client);
			drop(current_match);
			drop(clients_guard);

			let clients_guard = self.clients.read().await;
			let mut client = clients_guard.get(&addr).unwrap().write().await;
			if client.color == winner {
				client.score += 1;
			}
			client.current_match = None;
			client.color = Color::None;
			drop(client);

			let mut opponent = clients_guard.get(&opponent_addr).unwrap().write().await;
			if opponent.color == winner {
				opponent.score += 1;
			}
			opponent.current_match = None;
			opponent.color = Color::None;
			drop(opponent);
			matches_guard.remove(&current_match_id).unwrap();

			if !self.demo_mode && matches_guard.is_empty() {
				drop(matches_guard);
				drop(clients_guard);

				let mut tournament_guard = self.tournament.write().await;
				let tourney = tournament_guard.as_mut().unwrap();
				tourney.write().await.next(&self.clients, &self.matches, &self.observers).await;
				if tourney.read().await.is_completed() {
					*tournament_guard = None;
				}
			}

			return Ok(());
		}

		let connection_to_send = if !self.demo_mode {
			opponent_connection.clone()
		} else {
			tx.clone()
		};
		let column_to_use = if !self.demo_mode {
			column
		} else {
			crate::random_move(&current_match.board)
		};
		if self.demo_mode {
			let move_to_dispatch = current_match.move_to_dispatch.clone();
			current_match.ledger.push(move_to_dispatch);
			current_match.move_to_dispatch = (Color::Yellow, column_to_use);
			current_match.place_token(Color::Yellow, column_to_use);
		}

		let waiting =
			*self.waiting_timeout.read().await as i64 + (rand::rng().random_range(0..=50) - 25);
		let matches_move = self.matches.clone();
		let observers_move = self.observers.clone();
		let match_id_move = current_match.id;
		let demo_mode_move = self.demo_mode;
		current_match.wait_thread = Some(tokio::spawn(async move {
			tokio::time::sleep(tokio::time::Duration::from_millis(waiting as u64)).await;

			let mut matches_guard = matches_move.write().await;
			let mut current_match = matches_guard.get_mut(&match_id_move).unwrap().write().await;
			let move_to_dispatch = current_match.move_to_dispatch.clone();
			current_match.ledger.push(move_to_dispatch);
			current_match.move_to_dispatch = (Color::None, 0);

			if demo_mode_move {
				crate::broadcast_message(
					&current_match.viewers,
					&observers_move,
					&format!("GAME:MOVE:{}:{}", "demo", column_to_use),
				)
				.await;
			}

			drop(current_match);
			drop(matches_guard);

			let _ = crate::send(&connection_to_send, &format!("OPPONENT:{}", column_to_use));
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

		let _ = crate::send(&tx, to_send.as_str());
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
			let player1 = clients_guard.get(&a_match.player1).unwrap().read().await;
			let player2 = clients_guard.get(&a_match.player2).unwrap().read().await;
			to_send += a_match.id.to_string().as_str();
			to_send += ",";
			to_send += player1.username.as_str();
			to_send += ",";
			to_send += if player1.username == player2.username {
				"demo"
			} else {
				player2.username.as_str()
			};
			to_send += "|";
		}

		if !to_send.ends_with(":") {
			to_send.remove(to_send.len() - 1);
		}

		let _ = crate::send(&tx, to_send.as_str());
		Ok(())
	}

	pub async fn handle_game_watch(
		&self,
		tx: UnboundedSender<Message>,
		match_id: u32,
		addr: SocketAddr,
	) -> Result<(), anyhow::Error> {
		let result = crate::watch(&self.matches, match_id, addr).await;
		if result.is_err() {
			return Err(anyhow::anyhow!("ERROR:INVALID:WATCH"));
		}

		let clients_guard = self.clients.read().await;
		let matches_guard = self.matches.read().await;
		let the_match = matches_guard.get(&match_id).unwrap().read().await;
		let player1 = clients_guard.get(&the_match.player1).unwrap().read().await.username.clone();
		let mut player2 =
			clients_guard.get(&the_match.player2).unwrap().read().await.username.clone();
		if self.demo_mode {
			player2 = "demo".to_string();
		}
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

		let _ = crate::send(&tx, &message);
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
		let _ = crate::send(&tx, "ADMIN:AUTH:ACK");
		Ok(())
	}

	pub async fn handle_admin_kick(
		&self,
		tx: UnboundedSender<Message>,
		addr: SocketAddr,
		kick_username: String,
	) -> Result<(), anyhow::Error> {
		if !crate::auth_check(&self.admin, addr).await {
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
		tx: UnboundedSender<Message>,
		addr: SocketAddr,
		match_id: u32,
	) -> Result<(), anyhow::Error> {
		if !crate::auth_check(&self.admin, addr).await {
			return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
		}

		crate::terminate_match(
			match_id,
			&self.matches,
			&self.clients,
			&self.observers,
			self.demo_mode,
		)
		.await;

		if !self.demo_mode && self.matches.read().await.is_empty() {
			let mut tournament_guard = self.tournament.write().await;
			let tourney = tournament_guard.as_mut().unwrap();
			tourney.write().await.next(&self.clients, &self.matches, &self.observers).await;
			if tourney.read().await.is_completed() {
				*tournament_guard = None;
			}
		}
		Ok(())
	}

	pub async fn handle_tournament_start(
		&self,
		tx: UnboundedSender<Message>,
		addr: SocketAddr,
	) -> Result<(), anyhow::Error> {
		if !crate::auth_check(&self.admin, addr).await {
			return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
		}

		if self.tournament.read().await.is_some() || self.demo_mode {
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

		let mut tourney = match self.tournament_type.as_str() {
			"round_robin" => crate::tournaments::round_robin::RoundRobin::new(&ready_players),
			&_ => crate::tournaments::round_robin::RoundRobin::new(&ready_players),
		};
		tourney.start(&self.clients, &self.matches).await;

		let mut tournament_guard = self.tournament.write().await;
		*tournament_guard = Some(Arc::new(RwLock::new(tourney)));

		let _ = crate::send(&tx, "TOURNAMENT:START:ACK");
		Ok(())
	}

	pub async fn handle_tournament_cancel(
		&self,
		tx: UnboundedSender<Message>,
		addr: SocketAddr,
	) -> Result<(), anyhow::Error> {
		if !crate::auth_check(&self.admin, addr).await {
			return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
		}

		if self.tournament.read().await.is_none() || self.demo_mode {
			return Err(anyhow::anyhow!("ERROR:INVALID:TOURNAMENT"));
		}

		let mut tournament_guard = self.tournament.write().await;
		let tourney = tournament_guard.as_mut().unwrap();
		tourney.write().await.cancel(&self.clients, &self.matches, &self.observers).await;
		*tournament_guard = None;

		let _ = crate::send(&tx, "TOURNAMENT:CANCEL:ACK");
		Ok(())
	}

	pub async fn handle_tournament_wait(
		&self,
		tx: UnboundedSender<Message>,
		addr: SocketAddr,
		new_timeout: f64,
	) -> Result<(), anyhow::Error> {
		if !crate::auth_check(&self.admin, addr).await {
			return Err(anyhow::anyhow!("ERROR:INVALID:AUTH"));
		}

		*self.waiting_timeout.write().await = (new_timeout * 1000.0) as u64;
		Ok(())
	}

	pub async fn handle_get_move_wait(
		&self,
		tx: UnboundedSender<Message>,
	) -> Result<(), anyhow::Error> {
		let mut msg = "GET:MOVE_WAIT:".to_string();
		msg += &(*self.waiting_timeout.read().await as f64 / 1000f64).to_string();
		let _ = crate::send(&tx, &msg);
		Ok(())
	}

	pub async fn handle_get_tournament_status(
		&self,
		tx: UnboundedSender<Message>,
	) -> Result<(), anyhow::Error> {
		let status = self.tournament.read().await.is_some();
		if self.demo_mode {
			let _ = crate::send(&tx, "GET:TOURNAMENT_STATUS:DEMO");
		} else {
			let mut msg = "GET:TOURNAMENT_STATUS:".to_string();
			msg += status.to_string().as_str();
			let _ = crate::send(&tx, &msg);
		}
		Ok(())
	}
}

#[derive(PartialEq, Clone)]
pub enum Color {
	Red,
	Yellow,
	None,
}

#[derive(Clone)]
pub struct Client {
	pub username: String,
	pub connection: UnboundedSender<Message>,
	pub ready: bool,
	pub color: Color,
	pub current_match: Option<u32>,
	pub round_robin_id: u32,
	pub score: u32,
	pub addr: SocketAddr,
}

impl Client {
	pub fn new(username: String, connection: UnboundedSender<Message>, addr: SocketAddr) -> Client {
		Client {
			username,
			connection,
			ready: false,
			color: Color::None,
			current_match: None,
			round_robin_id: 0,
			score: 0,
			addr,
		}
	}
}

pub struct Match {
	pub id: u32,
	pub board: Vec<Vec<Color>>,
	pub viewers: Vec<SocketAddr>,
	pub ledger: Vec<(Color, usize)>,
	pub move_to_dispatch: (Color, usize),
	pub wait_thread: Option<tokio::task::JoinHandle<()>>,
	pub player1: SocketAddr,
	pub player2: SocketAddr,
}

impl Match {
	pub fn new(id: u32, player1: SocketAddr, player2: SocketAddr) -> Match {
		let first = if rand::rng().random_range(0..=1) == 0 {
			player1.to_string().parse().unwrap()
		} else {
			player2.to_string().parse().unwrap()
		};

		Match {
			id,
			board: vec![vec![Color::None; 6]; 7],
			viewers: Vec::new(),
			ledger: Vec::new(),
			move_to_dispatch: (Color::None, 0),
			wait_thread: None,
			player1: if player1 == first { player1 } else { player2 },
			player2: if player1 == first { player2 } else { player1 },
		}
	}

	pub fn place_token(&mut self, color: Color, column: usize) {
		for i in 0..6 {
			if self.board[column][i] == Color::None {
				self.board[column][i] = color;
				break;
			}
		}
	}
}
