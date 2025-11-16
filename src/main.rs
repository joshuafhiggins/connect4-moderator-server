mod types;

use crate::types::{Color, Match};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::RwLock;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{error, info, warn};
use types::Client;

type Clients = Arc<RwLock<HashMap<SocketAddr, Client<'static>>>>;
type Observers = Arc<RwLock<HashMap<SocketAddr, UnboundedSender<Message>>>>;
type Matches = Arc<RwLock<HashMap<u32, Match<'static>>>>;


#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:8080";
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on: {}", addr);

    let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
	let observers: Observers = Arc::new(RwLock::new(HashMap::new()));
    let matches: Matches = Arc::new(RwLock::new(HashMap::new()));

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(
            stream,
            addr,
            clients.clone(),
			observers.clone(),
            matches.clone(),
        ));
    }

    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    clients: Clients,
	observers: Observers,
    matches: Matches,
) -> Result<(), anyhow::Error> {
    info!("New WebSocket connection from: {}", addr);

    let ws_stream = accept_async(stream).await?;
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Store the client
    {
        observers
            .write()
            .await
            .insert(addr, tx.clone());
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
                        if requested_username == client.username {
							let _ = send(&tx, &format!("ERROR:INVALID:ID:{}", requested_username));
							is_taken = true;
							break;
                        }
                    }

                    if is_taken { continue; }

                    // not taken
					observers.write().await.remove(&addr);
                    clients.write().await.insert(addr.to_string().parse()?, Client::new(requested_username, tx.clone()));

					let _ = send(&tx, "CONNECT:ACK");
                }
				else if text.starts_with("READY") {
                    if clients.read().await.get(&addr).is_none() {
                        let _ = send(&tx, "ERROR:INVALID");
                        continue;
                    }

					if clients.read().await.get(&addr).unwrap().ready {
						let _ = send(&tx, "ERROR:INVALID");
						continue;
					}

					clients.write().await.get_mut(&addr).unwrap().ready = true;

					let _ = send(&tx,"READY:ACK");
                }
				else if text.starts_with("PLAY") {
					let read = clients.read().await;
					let client = read.get(&addr);

					// Check if client is valid
					if client.is_none() || client.unwrap().current_match.is_none() {
						let _ = send(&tx, "ERROR:INVALID:MOVE");
						continue;
					}

					let current_match = client.unwrap().current_match.unwrap();
					let current_player = if client.unwrap().username == current_match.player1.username
					{ current_match.player1 } else { current_match.player2 };
					let opponent = if client.unwrap().username == current_match.player1.username
					{ current_match.player2 } else { current_match.player1 };

					// Check if it's their move
					if (current_match.ledger.is_empty() &&
						current_match.first.username != client.unwrap().username) ||
						(client.unwrap().current_match.unwrap().ledger.last().unwrap().0.username == client.unwrap().username)
					{
						let _ = send(&tx, "ERROR:INVALID:MOVE");
						continue;
					}

					let column_parse = text.split(":").collect::<Vec<&str>>()[1].parse::<usize>();

					// Check if valid move
					if let Ok(column) = column_parse {
						if current_match.board[column][4] != Color::None {
							let _ = send(&tx, "ERROR:INVALID:MOVE");
							continue;
						}

						// Place it
						for i in 0..current_match.board[column].len() {
							if current_match.board[column][i] == Color::None {
								matches
									.write()
									.await
									.get_mut(&current_match.id).unwrap()
									.board[column][i] = current_player.color.clone();
								break;
							}
						}
					} else {
						let _ = send(&tx, "ERROR:INVALID:MOVE");
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
						if winner == current_player.color {
							let _ = send(&tx, "GAME:WINS");
							let _ = send(&opponent.connection, "GAME:LOSS");

						} else {
							let _ = send(&tx, "GAME:LOSS");
							let _ = send(&opponent.connection, "GAME:WINS");

						}
					}
					else if filled {
						let _ = send(&tx, "GAME:DRAW");
						let _ = send(&opponent.connection, "GAME:DRAW");
					}
					else {
						let _ = send(&opponent.connection, &format!("OPPONENT:{}", column_parse?));
					}

					// TODO: remove match from matchmaker
					// TODO: broadcast moves to viewers
				}
				else {
					let _ = send(&tx, "GAME:UNKNOWN");
				}
            }
            Ok(Message::Close(_)) => {
                info!("Client {} disconnected", addr);
                break;
            }
            Ok(_) => {
				let _ = send(&tx, "GAME:UNKNOWN");
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

async fn broadcast_message(observers: &Observers, msg: &str) {
    let observers = observers.read().await;
    for (_, tx) in observers.iter() {
		let _ = send(tx, msg);
	}
}

async fn watch(matches: &Matches, new_match_id: u32, addr: SocketAddr) {
	for a_match in &mut matches.write().await.values_mut() {
		let mut found = false;
		for i in 0..a_match.viewers.len() {
			if a_match.viewers[i] == addr {
				a_match.viewers.remove(i);
				found = true;
				break;
			}
		}

		if found { break; }
	}

	matches.write().await.get_mut(&new_match_id).unwrap().viewers.push(addr);
}

fn send(tx: &UnboundedSender<Message>, text: &str) -> Result<(), SendError<Message>> {
	tx.send(Message::text(text))
}
