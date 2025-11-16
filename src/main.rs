mod types;

use crate::types::{Color, MatchMaker, Role, AI};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{error, info, warn};
use types::Client;

type Clients = Arc<RwLock<HashMap<SocketAddr, Client<'static>>>>;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:8080";
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on: {}", addr);

    let mut clients: Clients = Arc::new(RwLock::new(HashMap::new()));
    let mut match_maker: Arc<RwLock<MatchMaker>> = Arc::new(RwLock::new(MatchMaker::new()));

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(
            stream,
            addr,
            clients.clone(),
            match_maker.clone(),
        ));
    }

    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    clients: Clients,
    match_maker: Arc<RwLock<MatchMaker>>,
) -> Result<(), anyhow::Error> {
    info!("New WebSocket connection from: {}", addr);

    let ws_stream = accept_async(stream).await?;
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Store the client
    {
        clients
            .write()
            .await
            .insert(addr, Client::new(String::new(), Role::Observer, tx));
    }

    // Spawn task to handle outgoing messages
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming messages
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                info!("Received text from {}: {}", addr, text);

                if text.starts_with("CONNECT") {
                    let requested_username = text.split(":").collect::<Vec<&str>>()[1].to_string();
                    let mut is_taken = false;
                    for client in clients.read().await.values() {
                        if let Some(username) = &client.username {
                            if *username == requested_username {
								let _ = clients
									.read()
									.await
									.get(&addr)
									.unwrap()
									.send(&format!("ERROR:INVALID:ID:{}", requested_username));
                                is_taken = true;
                                break;
                            }
                        }
                    }

                    if is_taken { continue; }

                    // not taken
                    clients.write().await.get_mut(&addr).unwrap().role = Role::Player;
                    clients.write().await.get_mut(&addr).unwrap().username = Some(requested_username);

					let _ = clients
						.read()
						.await
						.get(&addr)
						.unwrap()
						.send("CONNECT:ACK");
                }
				else if text.starts_with("READY") {
                    if clients.read().await.get(&addr).unwrap().role != Role::Player {
                        let _ = clients
                            .read()
                            .await
                            .get(&addr)
                            .unwrap()
                            .send("ERROR:INVALID");
                        continue;
                    }

					let mut already_ready = false;
					for ready_player in &match_maker.read().await.ready_players {
						if ready_player.username.eq(clients.read().await.get(&addr).unwrap().username.as_ref().unwrap()) {
							let _ = clients
								.read()
								.await
								.get(&addr)
								.unwrap()
								.send("ERROR:INVALID");
							already_ready = true;
							break;
						}
					}

					if already_ready { continue; }

                    match_maker.write().await.ready_players.push(AI::new(
                        clients
                            .read()
                            .await
                            .get(&addr)
                            .unwrap()
                            .username
                            .as_ref()
                            .unwrap(),
                        Color::None,
						addr.to_string()
                    ));

					let _ = clients
						.read()
						.await
						.get(&addr)
						.unwrap()
						.send("READY:ACK");
                }
				else if text.starts_with("PLAY") {
					let read = clients.read().await;
					let client = read.get(&addr).unwrap();

					// Check if client is valid
					if client.role != Role::Player || client.current_match.is_none() {
						let _ = clients
							.read()
							.await
							.get(&addr)
							.unwrap()
							.send("ERROR:INVALID:MOVE");
						continue;
					}

					let current_match = client.current_match.unwrap();
					let ai = if *client.username.as_ref().unwrap() == current_match.player1.username
					{ &current_match.player1 } else { &current_match.player2 };
					let opponent = if *client.username.as_ref().unwrap() == current_match.player1.username
					{ &current_match.player2 } else { &current_match.player1 };

					// Check if it's their move
					if (current_match.ledger.is_empty() &&
						current_match.first.username != *client.username.as_ref().unwrap()) ||
						(client.current_match.unwrap().ledger.last().unwrap().0.username == *client.username.as_ref().unwrap())
					{
						let _ = clients
							.read()
							.await
							.get(&addr)
							.unwrap()
							.send("ERROR:INVALID:MOVE");
						continue;
					}

					// Check if valid move
					if let Ok(column) = text.split(":").collect::<Vec<&str>>()[1].parse::<usize>() {
						if current_match.board[column][4] != Color::None {
							let _ = clients
								.read()
								.await
								.get(&addr)
								.unwrap()
								.send("ERROR:INVALID:MOVE");
							continue;
						}

						// Place it
						for i in 0..current_match.board[column].len() {
							if current_match.board[column][i] == Color::None {
								match_maker.write().await.matches.get_mut(&current_match.id).unwrap().board[column][i] = ai.color.clone();
								break;
							}
						}
					} else {
						let _ = clients
							.read()
							.await
							.get(&addr)
							.unwrap()
							.send("ERROR:INVALID:MOVE");
						continue;
					}

					// Check game end conditions
					let (winner, filled) = {
						let mut result = (Color::None, false);

						let mut any_empty = true;
						for x in 0..6 {
							for y in 0..5 {
								let color = &current_match.board[x][y];
								let mut horizontal_end = true;
								let mut vertical_end = true;
								let mut diagonal_end = true;

								if any_empty && *color == Color::None {
									any_empty = false;
								}

								for i in 0..4 {
									if x + 3 < 6 && current_match.board[x + i][y] != *color {
										horizontal_end = false;
									}

									if y + 3 < 5 && current_match.board[x + i][y] != *color {
										vertical_end = false;
									}

									if x + 3 < 6 && y + 3 < 5 && current_match.board[x + i][y + i] != *color {
										diagonal_end = false;
									}
								}

								if horizontal_end || vertical_end || diagonal_end { result = (color.clone(), false) }
							}
						}

						if any_empty && result.0 == Color::None { result.1 = true; }

						result
					};

					if winner != Color::None {
						if winner == ai.color {
							let _ = clients
								.read()
								.await
								.get(&addr)
								.unwrap()
								.send("GAME:WINS");

							let _ = clients
								.read()
								.await
								.get(&SocketAddr::from_str(&opponent.addr)?)
								.unwrap()
								.send("GAME:LOSS");
						} else {
							let _ = clients
								.read()
								.await
								.get(&addr)
								.unwrap()
								.send("GAME:LOSS");

							let _ = clients
								.read()
								.await
								.get(&SocketAddr::from_str(&opponent.addr)?)
								.unwrap()
								.send("GAME:WINS");
						}
					}
					else if filled {
						let _ = clients
							.read()
							.await
							.get(&addr)
							.unwrap()
							.send("GAME:DRAW");

						let _ = clients
							.read()
							.await
							.get(&SocketAddr::from_str(&opponent.addr)?)
							.unwrap()
							.send("GAME:DRAW");
					}
					else {
						let _ = clients
							.read()
							.await
							.get(&SocketAddr::from_str(&opponent.addr)?)
							.unwrap()
							.send(&format!("OPPONENT:{}", text.split(":").collect::<Vec<&str>>()[1].parse::<usize>()?));
					}

					// TODO: remove match from matchmaker
					// TODO: broadcast moves to viewers
				}
				else {
                    let _ = clients
                        .read()
                        .await
                        .get(&addr)
                        .unwrap()
                        .send("ERROR:UNKNOWN");
                }
            }
            Ok(Message::Close(_)) => {
                info!("Client {} disconnected", addr);
                break;
            }
            Ok(_) => {
                let _ = clients
                    .read()
                    .await
                    .get(&addr)
                    .unwrap()
                    .send("ERROR:UNKNOWN");
            }
            Err(e) => {
                error!("WebSocket error for {}: {}", addr, e);
                break;
            }
        }
    }

    // Clean up
    send_task.abort();

    // TODO: Remove and terminate any matches

    clients.write().await.remove(&addr);

    info!("Client {} removed", addr);

    Ok(())
}

async fn broadcast_message(clients: &Clients, msg: Message) {
    let clients = clients.read().await;
    for (_, client) in clients.iter() {
        if client.role == Role::Admin || client.role == Role::Observer {
            client.connection.send(msg.clone()).ok();
        }
    }
}
