mod types;

use crate::types::{Color, Match, Tournament};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::RwLock;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{error, info, warn};
use types::Client;

type Clients = Arc<RwLock<HashMap<SocketAddr, Arc<RwLock<Client>>>>>;
type Usernames = Arc<RwLock<HashMap<String, SocketAddr>>>;
type Observers = Arc<RwLock<HashMap<SocketAddr, UnboundedSender<Message>>>>;
type Matches = Arc<RwLock<HashMap<u32, Arc<RwLock<Match>>>>>;

// TODO: allow random player1 in demo mode

struct Server {
	clients: Clients,
	usernames: Usernames,
	observers: Observers,
	matches: Matches,
	admin: Arc<RwLock<Option<SocketAddr>>>,
	admin_password: Arc<String>,
	tournament: Arc<RwLock<Option<Tournament>>>,
	waiting_timeout: Arc<RwLock<u64>>,
	demo_mode: bool,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let demo_mode = args.get(1).is_some() && args.get(1).unwrap() == "demo";
	let admin_password = env::var("ADMIN_AUTH").unwrap_or_else(|_| String::from("admin"));
	info!("Admin password: {}", admin_password);
	let admin_password = Arc::new(admin_password);

    let addr = "0.0.0.0:8080";
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on: {}", addr);

    let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
	let usernames: Usernames = Arc::new(RwLock::new(HashMap::new()));
    let observers: Observers = Arc::new(RwLock::new(HashMap::new()));
    let matches: Matches = Arc::new(RwLock::new(HashMap::new()));
    let admin: Arc<RwLock<Option<SocketAddr>>> = Arc::new(RwLock::new(None));
	let tournament: Arc<RwLock<Option<Tournament>>> = Arc::new(RwLock::new(None));
	let waiting_timeout: Arc<RwLock<u64>> = Arc::new(RwLock::new(5000));

	let server_data = Arc::new(
		Server {
			clients,
			usernames,
			observers,
			matches,
			admin,
			admin_password,
			tournament,
			waiting_timeout,
			demo_mode
		}
	);

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(
            stream,
            addr,
            server_data.clone(),
        ));
    }

    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    sd: Arc<Server>,
) -> Result<(), anyhow::Error> {
    info!("New WebSocket connection from: {}", addr);

    let ws_stream = accept_async(stream).await?;
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Store the client
    sd.observers.write().await.insert(addr, tx.clone());

    // Spawn task to handle outgoing messages
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg.clone()).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming messages
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                info!("Received text from {}: {}", addr, text);

                if text.starts_with("CONNECT:") {
                    let requested_username = text.split(":").collect::<Vec<&str>>()[1].to_string();

                    if requested_username.is_empty() {
                        let _ = send(&tx, &format!("ERROR:INVALID:ID:{}", requested_username));
                        continue;
                    }

                    let mut is_taken = false;
                    for client in sd.clients.read().await.values() {
                        if requested_username == client.read().await.username {
                            let _ = send(&tx, &format!("ERROR:INVALID:ID:{}", requested_username));
                            is_taken = true;
                            break;
                        }
                    }

                    if is_taken {
                        continue;
                    }

                    // not taken
					sd.observers.write().await.remove(&addr);
					sd.usernames.write().await.insert(requested_username.clone(), addr);
					sd.clients.write().await.insert(
                        addr.to_string().parse()?,
                        Arc::new(RwLock::new(Client::new(
                            requested_username,
                            tx.clone(),
                            addr.to_string().parse()?,
                        ))),
                    );

                    let _ = send(&tx, "CONNECT:ACK");
                }
				else if text == "READY" {
					let clients_guard = sd.clients.read().await;
                    if clients_guard.get(&addr).is_none() {
                        let _ = send(&tx, "ERROR:INVALID");
                        continue;
                    }

                    if clients_guard.get(&addr).unwrap().read().await.ready {
                        let _ = send(&tx, "ERROR:INVALID");
                        continue;
                    }

					let mut client = clients_guard.get(&addr).unwrap().write().await;
                    client.ready = true;
                    let _ = send(&tx, "READY:ACK");

                    if sd.demo_mode {
                        let match_id: u32 = gen_match_id(&sd.matches).await;
                        let new_match = Arc::new(RwLock::new(Match::new(
                            match_id,
                            addr.to_string().parse()?,
                            addr.to_string().parse()?,
                        )));
						sd.matches.write().await.insert(match_id, new_match.clone());
                        client.ready = false;
                        client.current_match = Some(match_id);
                        client.color = Color::Red;
                        let _ = send(&tx, "GAME:START:1");
                    }
                }
				else if text.starts_with("PLAY:") {
                    let clients_guard = sd.clients.read().await;
                    let client = clients_guard.get(&addr);

                    // Check if client is valid
                    if client.is_none() || client.unwrap().read().await.current_match.is_none()
                    {
                        let _ = send(&tx, "ERROR:INVALID:MOVE");
                        continue;
                    }
                    let client = client.unwrap().read().await;

                    let matches_guard = sd.matches.read().await;
                    let current_match =
                        matches_guard.get(&client.current_match.unwrap()).unwrap().read().await;

					let opponent_addr =
					if addr == current_match.player1 {
						current_match.player1
					} else {
						current_match.player2
					};

					let opponent_connection =
					if addr == current_match.player1 {
						clients_guard.get(&current_match.player2).unwrap().read().await.connection.clone()
					} else {
						clients_guard.get(&current_match.player1).unwrap().read().await.connection.clone()
					};

					let opponent_username =
					if addr == current_match.player1 {
						clients_guard.get(&current_match.player2).unwrap().read().await.username.clone()
					} else {
						clients_guard.get(&current_match.player1).unwrap().read().await.username.clone()
					};


                    // Check if it's their move
                    if (current_match.ledger.is_empty() && current_match.player1 != addr) ||
						(current_match.ledger.last().is_some() && current_match.ledger.last().unwrap().0 == client.color)
                    {
                        let _ = send(&tx, "ERROR:INVALID:MOVE");
                        continue;
                    }

                    let column_parse = text.split(":").collect::<Vec<&str>>()[1].parse::<usize>();

                    drop(current_match);
                    drop(matches_guard);

                    let mut matches_guard = sd.matches.write().await;
                    let mut current_match = matches_guard
                        .get_mut(&client.current_match.unwrap())
                        .unwrap()
                        .write()
                        .await;

                    // Check if valid move
					let mut invalid = false;
                    if let Ok(column) = column_parse {
                        if column >= 7 {
                            let _ = send(&tx, "ERROR:INVALID:MOVE");
							invalid = true;
                        }

                        if current_match.board[column][5] != Color::None && !invalid {
                            let _ = send(&tx, "ERROR:INVALID:MOVE");
							invalid = true;
                        }


                    } else {
                        let _ = send(&tx, "ERROR:INVALID:MOVE");
						invalid = true;
                    }

					// Terminate games if a player makes an invalid move
					if invalid {
						let current_match_id = current_match.id;

						if sd.demo_mode {
							terminate_match(current_match_id, &sd.matches, &sd.clients, &sd.observers, sd.demo_mode).await;
							tx.send(Message::Close(None))?;
						} else {
							let _ = send(&tx, "GAME:LOSS");
							let _ = send(&opponent_connection, "GAME:WINS");
							broadcast_message(&current_match.viewers, &sd.observers, &format!("GAME:WIN:{}", opponent_username)).await;

							drop(client);
							drop(current_match);
							drop(clients_guard);

							let mut clients_guard = sd.clients.write().await;
							let mut client = clients_guard.get_mut(&addr).unwrap().write().await;
							client.current_match = None;
							client.color = Color::None;
							drop(client);

							let mut opponent = clients_guard.get_mut(&opponent_addr).unwrap().write().await;
							opponent.current_match = None;
							opponent.color = Color::None;
							opponent.score += 1;
							drop(opponent);

							matches_guard.remove(&current_match_id).unwrap();
						}
						continue;
					} else {
						// Place it
						current_match.place_token(client.color.clone(), column_parse.clone()?);
						current_match.move_to_dispatch = (client.color.clone(), column_parse.clone()?);
					}

                    // broadcast the move to viewers
                    broadcast_message(
                        &current_match.viewers,
                        &sd.observers,
                        &format!("GAME:MOVE:{}:{}", client.username, column_parse.clone()?),
                    )
                    .await;

                    // Check game end conditions
                    let (winner, filled) = end_game_check(&current_match.board);

                    if winner != Color::None {
                        if winner == client.color {
                            let _ = send(&tx, "GAME:WINS");
                            if !sd.demo_mode {
                                let _ = send(&opponent_connection, "GAME:LOSS");
                            }
                            broadcast_message(
                                &current_match.viewers,
                                &sd.observers,
                                &format!("GAME:WIN:{}", client.username),
                            )
                            .await;
                        } else {
                            let _ = send(&tx, "GAME:LOSS");
                            if !sd.demo_mode {
                                let _ = send(&opponent_connection, "GAME:WINS");
                            }
                            broadcast_message(
                                &current_match.viewers,
                                &sd.observers,
                                &format!("GAME:WIN:{}", opponent_username),
                            )
                            .await;
                        }
                    } else if filled {
                        let _ = send(&tx, "GAME:DRAW");
                        if !sd.demo_mode {
                            let _ = send(&opponent_connection, "GAME:DRAW");
                        }
                        broadcast_message(&current_match.viewers, &sd.observers, "GAME:DRAW").await;
                    }

                    // remove match from matchmaker
                    if winner != Color::None || filled {
                        let current_match_id = current_match.id;

                        drop(client);
                        drop(current_match);
                        drop(clients_guard);

                        let mut clients_guard = sd.clients.write().await;
                        let mut client = clients_guard.get_mut(&addr).unwrap().write().await;
						if client.color == winner {
							client.score += 1;
						}
                        client.current_match = None;
						client.color = Color::None;
                        drop(client);

                        let mut opponent =
                            clients_guard.get_mut(&opponent_addr).unwrap().write().await;

                        if !sd.demo_mode {
                            opponent.current_match = None;
							if opponent.color == winner {
								opponent.score += 1;
							}
							opponent.color = Color::None;
                        }
                        matches_guard.remove(&current_match_id).unwrap();
						drop(opponent);

						if !sd.demo_mode && matches_guard.is_empty() {
							let mut tournament_guard = sd.tournament.write().await;
							let tourney = tournament_guard.as_mut().unwrap();
							tourney.next();
							if tourney.is_completed {
								// Send scores
								let mut player_scores: Vec<(String, u32)> = Vec::new();
								for (_, player_addr) in tourney.players.iter() {
									let mut player = clients_guard.get_mut(player_addr).unwrap().write().await;
									player_scores.push((player.username.clone(), player.score));
									player.score = 0;
									player.round_robin_id = 0;
								}

								player_scores.sort_by(|a, b| b.1.cmp(&a.1));

								let mut message = "TOURNAMENT:END:".to_string();
								for (player, score) in player_scores.iter() {
									message.push_str(&format!("{},{}|", player, score))
								}
								message.pop();

								broadcast_message_all_observers(&sd.observers, &message).await;
							} else {
								// Create next matches
								// TODO: Make this a function
								for (i, id) in tourney.top_half.iter().enumerate() {
									let player1_addr = tourney.players.get(id).unwrap();
									let player2_addr = tourney.players.get(tourney.bottom_half.get(i).unwrap());

									if player2_addr.is_none() { continue; }
									let player2_addr = player2_addr.unwrap();

									let match_id: u32 = gen_match_id(&sd.matches).await;
									let new_match = Arc::new(RwLock::new(Match::new(
										match_id,
										*player1_addr,
										*player2_addr,
									)));

									let match_guard = new_match.read().await;
									let mut player1 = clients_guard.get_mut(player1_addr).unwrap().write().await;

									player1.current_match = Some(match_id);
									player1.ready = false;

									if match_guard.player1 == *player1_addr {
										player1.color = Color::Red;
										let _ = send(&player1.connection, "GAME:START:1");
									} else {
										player1.color = Color::Blue;
										let _ = send(&player1.connection, "GAME:START:0");
									}

									drop(player1);

									let mut player2 = clients_guard.get_mut(player2_addr).unwrap().write().await;

									player2.current_match = Some(match_id);
									player2.ready = false;

									if match_guard.player1 == *player2_addr {
										player2.color = Color::Red;
										let _ = send(&player2.connection, "GAME:START:1");
									} else {
										player2.color = Color::Blue;
										let _ = send(&player2.connection, "GAME:START:0");
									}

									drop(player2);

									matches_guard.insert(match_id, new_match.clone());
								}
							}
						}

                        continue;
                    }

					let connection_to_send = if !sd.demo_mode { opponent_connection.clone() } else { tx.clone() };
					let column = if !sd.demo_mode { column_parse.clone()? } else { random_move(&current_match.board) };
					if sd.demo_mode {
						let move_to_dispatch = current_match.move_to_dispatch.clone();
						current_match.ledger.push(move_to_dispatch);
						current_match.move_to_dispatch = (Color::Blue, column);
						current_match.place_token(Color::Blue, column);
					}

					let waiting = *sd.waiting_timeout.read().await as i64 + (rand::rng().random_range(0..=50) - 25);
					let matches_move = sd.matches.clone();
					let observers_move = sd.observers.clone();
					let match_id_move = current_match.id;
					let demo_mode_move = sd.demo_mode;
					current_match.wait_thread = Some(tokio::spawn(async move {
						tokio::time::sleep(tokio::time::Duration::from_millis(waiting as u64)).await;

						let mut matches_guard = matches_move.write().await;
						let mut current_match = matches_guard.get_mut(&match_id_move).unwrap().write().await;
						let move_to_dispatch = current_match.move_to_dispatch.clone();
						current_match.ledger.push(move_to_dispatch);
						current_match.move_to_dispatch = (Color::None, 0);

						if demo_mode_move {
							broadcast_message(
								&current_match.viewers,
								&observers_move,
								&format!("GAME:MOVE:{}:{}", "demo", column),
							).await;
						}

						drop(current_match);
						drop(matches_guard);

						let _ = send(&connection_to_send, &format!("OPPONENT:{}", column));
					}));
                }

				else if text == "PLAYER:LIST" {
					let clients_guard = sd.clients.read().await;
					let mut to_send = "PLAYER:LIST:".to_string();
					for client_guard in clients_guard.values() {
						let player = client_guard.read().await;
						to_send += player.username.as_str(); to_send += ",";
						to_send += if player.ready { "true" } else { "false" }; to_send += ",";
						to_send += if player.current_match.is_some() { "true" } else { "false" };
						to_send += "|";
					}

					to_send.remove(to_send.len() - 1);

					let _ = send(&tx, to_send.as_str());
				}
				else if text == "GAME:LIST" {
					let matches_guard = sd.matches.read().await;
					let clients_guard = sd.clients.read().await;
					let mut to_send = "GAME:LIST:".to_string();
					for match_guard in matches_guard.values() {
						let a_match = match_guard.read().await;
						let player1 = clients_guard.get(&a_match.player1).unwrap().read().await;
						let player2 = clients_guard.get(&a_match.player2).unwrap().read().await;
						to_send += a_match.id.to_string().as_str();
						to_send += ","; to_send += player1.username.as_str(); to_send += ",";
						to_send += if player1.username == player2.username { "demo" } else { player2.username.as_str() };
						to_send += "|";
					}

					to_send.remove(to_send.len() - 1);

                    let _ = send(&tx, to_send.as_str());
                }
				else if text.starts_with("GAME:WATCH:") {
					let match_id_parse = text.split(":").collect::<Vec<&str>>()[2].parse::<u32>();
					match match_id_parse {
						Ok(match_id) => {
							let result = watch(&sd.matches, match_id, addr).await;
							if result.is_err() { let _ = send(&tx, "ERROR:INVALID:WATCH"); continue; }

							let clients_guard = sd.clients.read().await;
							let matches_guard = sd.matches.read().await;
							let the_match = matches_guard.get(&match_id).unwrap().read().await;
							let player1 = clients_guard.get(&the_match.player1).unwrap().read().await.username.clone();
							let mut player2 = clients_guard.get(&the_match.player2).unwrap().read().await.username.clone();
							if sd.demo_mode { player2 = "demo".to_string(); }
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
						}
						Err(_) => { let _ = send(&tx, "ERROR:INVALID:WATCH"); continue; }
					}
                }

				else if text.starts_with("ADMIN:AUTH:") {
					if sd.admin.read().await.is_some() {
						let _ = send(&tx, "ERROR:INVALID:AUTH");
						continue;
					}

					let password_parse = text.split(":").collect::<Vec<&str>>()[2];
					if password_parse != *sd.admin_password {
						let _ = send(&tx, "ERROR:INVALID:AUTH");
						continue;
					}

					let mut admin_guard = sd.admin.write().await;
					*admin_guard = Some(addr.to_string().parse()?);
					let _ = send(&tx, "ADMIN:AUTH:ACK");
                }
				else if text.starts_with("ADMIN:KICK:") {
					if !auth_check(&sd.admin, addr).await {
						let _ = send(&tx, "ERROR:INVALID:AUTH");
					}

					let kick_username = text.split(":").collect::<Vec<&str>>()[2];

					let usernames_guard = sd.usernames.read().await;
					let clients_guard = sd.clients.read().await;

					let kick_addr_result = usernames_guard.get(kick_username);
					match kick_addr_result {
						Some(kick_addr) => {
							let kick_client = clients_guard.get(kick_addr).unwrap().read().await;
							kick_client.connection.send(Message::Close(None))?;
						},
						None => {
							let _ = send(&tx, "ERROR:INVALID:KICK");
							continue
						}
					}
                }
				else if text.starts_with("GAME:TERMINATE:") {
					if !auth_check(&sd.admin, addr).await {
						let _ = send(&tx, "ERROR:INVALID:AUTH");
					}

					let match_id_parse = text.split(":").collect::<Vec<&str>>()[2].parse::<u32>();
					if match_id_parse.is_err() { let _ = send(&tx, "ERROR:INVALID:TERMINATE"); continue; }

					terminate_match(match_id_parse?, &sd.matches, &sd.clients, &sd.observers, sd.demo_mode).await;
                }

				else if text == "TOURNAMENT:START" {
					if !auth_check(&sd.admin, addr).await {
						let _ = send(&tx, "ERROR:INVALID:AUTH");
					}

					if sd.tournament.read().await.is_some() || sd.demo_mode {
						let _ = send(&tx, "ERROR:INVALID:TOURNAMENT");
						continue;
					}

					let mut clients_guard = sd.clients.write().await;
					let mut ready_players = Vec::new();
					for (client_addr, client_guard) in clients_guard.iter_mut() {
						if client_guard.read().await.ready {
							ready_players.push(*client_addr);
						}
					}

					if ready_players.len() < 3 {
						let _ = send(&tx, "ERROR:INVALID:TOURNAMENT");
						continue;
					}

					let tourney = Tournament::new(&ready_players);

					let mut tournament_guard = sd.tournament.write().await;
					*tournament_guard = Some(tourney.clone());

					for (i, id) in tourney.top_half.iter().enumerate() {
						let player1_addr = tourney.players.get(id).unwrap();
						let player2_addr = tourney.players.get(tourney.bottom_half.get(i).unwrap()).unwrap();
						let match_id: u32 = gen_match_id(&sd.matches).await;
						let new_match = Arc::new(RwLock::new(Match::new(
							match_id,
							*player1_addr,
							*player2_addr,
						)));

						let match_guard = new_match.read().await;
						let mut player1 = clients_guard.get_mut(player1_addr).unwrap().write().await;

						player1.current_match = Some(match_id);
						player1.ready = false;

						if match_guard.player1 == *player1_addr {
							player1.color = Color::Red;
							let _ = send(&player1.connection, "GAME:START:1");
						} else {
							player1.color = Color::Blue;
							let _ = send(&player1.connection, "GAME:START:0");
						}

						drop(player1);

						let mut player2 = clients_guard.get_mut(player2_addr).unwrap().write().await;

						player2.current_match = Some(match_id);
						player2.ready = false;

						if match_guard.player1 == *player2_addr {
							player2.color = Color::Red;
							let _ = send(&player2.connection, "GAME:START:1");
						} else {
							player2.color = Color::Blue;
							let _ = send(&player2.connection, "GAME:START:0");
						}

						drop(player2);

						sd.matches.write().await.insert(match_id, new_match.clone());
					}

					let _ = send(&tx, "TOURNAMENT:START:ACK");
				}
				// TODO: Cancel Tournament
				else if text.starts_with("TOURNAMENT:WAIT:") {
					if !auth_check(&sd.admin, addr).await {
						let _ = send(&tx, "ERROR:INVALID:AUTH");
					}

					let new_timeout = text.split(":").collect::<Vec<&str>>()[2].parse::<f64>()?;
					*sd.waiting_timeout.write().await = (new_timeout * 1000.0) as u64;
				}

				else {
                    let _ = send(&tx, "ERROR:UNKNOWN");
                }
            }
            Ok(Message::Close(_)) => {
                info!("Client {} disconnected", addr);
                break;
            }
            Ok(Message::Ping(b)) => { let _ = tx.send(Message::Pong(b)); }
            Ok(Message::Binary(_)) => { let _ = send(&tx, "ERROR:UNKNOWN"); }
			Ok(_) => { info!("Received pong/frame? Something fishy is happening") },
            Err(e) => {
                error!("WebSocket error for {}: {}", addr, e);
                break;
            },
        }
    }

    // Clean up
    send_task.abort();

	// TODO: Support reconnecting behaviors

    // Remove and terminate any matches
	// We may not be a client disconnecting, do this check
	if sd.clients.read().await.get(&addr).is_some() {
		if let Some(match_id) = sd.clients.read().await.get(&addr).unwrap().read().await.current_match {
			terminate_match(match_id, &sd.matches, &sd.clients, &sd.observers, sd.demo_mode).await;
		}
		let client = sd.clients.write().await.remove(&addr).unwrap();
		let username = client.read().await.username.clone();
		sd.usernames.write().await.remove(&username);
	}

	sd.observers.write().await.remove(&addr);

    let mut admin_guard = sd.admin.write().await;
    if let Some(admin_addr) = *admin_guard {
        if admin_addr == addr {
            *admin_guard = None;
        }
    }
    drop(admin_guard);

    info!("Client {} removed", addr);

    Ok(())
}

fn end_game_check(board: &[Vec<Color>]) -> (Color, bool) {
	let mut result = (Color::None, false);

	let mut any_empty = true;
	for x in 0..7 {
		for y in 0..6 {
			let color = board[x][y].clone();
			let mut horizontal_end = true;
			let mut vertical_end = true;
			let mut diagonal_end_up = true;
			let mut diagonal_end_down = true;

			if any_empty && color == Color::None {
				any_empty = false;
			}

			for i in 0..4 {
				if x + i >= 7 || board[x + i][y] != color && horizontal_end {
					horizontal_end = false;
				}

				if y + i >= 6 || board[x][y + i] != color && vertical_end {
					vertical_end = false;
				}

				if x + i >= 7 || y + i >= 6 || board[x + i][y + i] != color && diagonal_end_up {
					diagonal_end_up = false;
				}

				if x + i >= 7 || (y as i32 - i as i32) < 0 ||
					board[x + i][y - i] != color && diagonal_end_down {
					diagonal_end_down = false;
				}
			}

			if horizontal_end || vertical_end || diagonal_end_up || diagonal_end_down {
				result = (color.clone(), false);
				break;
			}
		}
		if result.0 != Color::None {
			break;
		}
	}

	if any_empty && result.0 == Color::None {
		result.1 = true;
	}

	result
}

async fn broadcast_message(addrs: &Vec<SocketAddr>, observers: &Observers, msg: &str) {
    for addr in addrs {
		let observers_guard = observers.read().await;
		let tx = observers_guard.get(addr);
		if tx.is_none() { continue; }
        let _ = send(tx.unwrap(), msg);
    }
}

async fn broadcast_message_all_observers(observers: &Observers, msg: &str) {
	let observers_guard = observers.read().await;
	for (_, tx) in observers_guard.iter() {
		let _ = send(tx, msg);
	}
}

async fn watch(matches: &Matches, new_match_id: u32, addr: SocketAddr) -> Result<(), String> {
	let matches_guard = matches.read().await;

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

async fn auth_check(admin: &Arc<RwLock<Option<SocketAddr>>>, addr: SocketAddr) -> bool {
	if admin.read().await.is_none() || admin.read().await.unwrap() != addr {
		return false;
	}
	true
}

async fn gen_match_id(matches: &Matches) -> u32 {
	let matches_guard = matches.read().await;
	let mut result = rand::rng().random_range(100000..=999999);
	while matches_guard.get(&result).is_some() {
		result = rand::rng().random_range(100000..=999999);
	}
	result
}

async fn terminate_match(match_id: u32, matches: &Matches, clients: &Clients, observers: &Observers, demo_mode: bool) {
	let matches_guard = matches.read().await;
	let the_match = matches_guard.get(&match_id);
	if the_match.is_none() {
		error!("Tried to call terminate_match on invalid matchID: {}", match_id);
	}
	let the_match = the_match.unwrap().read().await;

	if the_match.wait_thread.is_some() {
		the_match.wait_thread.as_ref().unwrap().abort();
	}

	broadcast_message(&the_match.viewers, observers, "GAME:TERMINATED").await;

	let clients_guard = clients.read().await;
	let mut player1 = clients_guard.get(&the_match.player1).unwrap().write().await;
	let _ = send(&player1.connection, "GAME:TERMINATED");
	player1.current_match = None;
	player1.color = Color::None;
	drop(player1);

	if !demo_mode {
		let mut player2 = clients_guard.get(&the_match.player2).unwrap().write().await;
		let _ = send(&player2.connection, "GAME:TERMINATED");
		player2.current_match = None;
		player2.color = Color::None;
		drop(player2);
	}

	drop(clients_guard);

	drop(the_match);
	drop(matches_guard);

	matches.write().await.remove(&match_id);
}

fn random_move(board: &[Vec<Color>]) -> usize {
	let mut random = rand::rng().random_range(0..7);
	while board[random][5] != Color::None {
		random = rand::rng().random_range(0..7);
	}

	random
}

fn send(tx: &UnboundedSender<Message>, text: &str) -> Result<(), SendError<Message>> {
    tx.send(Message::text(text))
}
