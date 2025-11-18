mod random_ai;
mod types;

use crate::random_ai::random_move;
use crate::types::{Color, Match};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
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

type Clients = Arc<RwLock<HashMap<SocketAddr, Arc<RwLock<Client>>>>>;
type Observers = Arc<RwLock<HashMap<SocketAddr, UnboundedSender<Message>>>>;
type Matches = Arc<RwLock<HashMap<u32, Arc<RwLock<Match>>>>>;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    let demo_mode = args.get(1).is_some() && args.get(1).unwrap() == "demo";

    let addr = "0.0.0.0:8080";
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on: {}", addr);

    let clients: Clients = Arc::new(RwLock::new(HashMap::new()));
    let observers: Observers = Arc::new(RwLock::new(HashMap::new()));
    let matches: Matches = Arc::new(RwLock::new(HashMap::new()));
    let admin: Arc<RwLock<Option<SocketAddr>>> = Arc::new(RwLock::new(None));

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(
            stream,
            addr,
            clients.clone(),
            observers.clone(),
            matches.clone(),
            admin.clone(),
            demo_mode,
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
    admin: Arc<RwLock<Option<SocketAddr>>>,
    demo_mode: bool,
) -> Result<(), anyhow::Error> {
    info!("New WebSocket connection from: {}", addr);

    let ws_stream = accept_async(stream).await?;
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Store the client
    observers.write().await.insert(addr, tx.clone());

    // Spawn task to handle outgoing messages
    let send_task = tokio::spawn(async move {
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

                if text.starts_with("CONNECT:") {
                    let requested_username = text.split(":").collect::<Vec<&str>>()[1].to_string();

                    if requested_username.is_empty() {
                        let _ = send(&tx, &format!("ERROR:INVALID:ID:{}", requested_username));
                        continue;
                    }

                    let mut is_taken = false;
                    for client in clients.read().await.values() {
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
                    observers.write().await.remove(&addr);
                    clients.write().await.insert(
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
                    if clients.read().await.get(&addr).is_none() {
                        let _ = send(&tx, "ERROR:INVALID");
                        continue;
                    }

                    if clients.read().await.get(&addr).unwrap().read().await.ready {
                        let _ = send(&tx, "ERROR:INVALID");
                        continue;
                    }

                    clients.write().await.get_mut(&addr).unwrap().write().await.ready = true;

                    let _ = send(&tx, "READY:ACK");

                    if demo_mode {
                        let match_id: u32 = rand::rng().random_range(100000..=999999);
                        let new_match = Arc::new(RwLock::new(Match::new(
                            match_id,
                            addr.to_string().parse()?,
                            addr.to_string().parse()?,
                        )));
                        matches.write().await.insert(match_id, new_match.clone());
                        clients.write().await.get_mut(&addr).unwrap().write().await.ready = false;
                        clients.write().await.get_mut(&addr).unwrap().write().await.current_match =
                            Some(match_id);
                        clients.write().await.get_mut(&addr).unwrap().write().await.color =
                            Color::Red;
                        let _ = send(&tx, "GAME:START:1");
                    }
                }
				else if text.starts_with("PLAY:") {
                    let clients_guard = clients.read().await;
                    let client_option = clients_guard.get(&addr);

                    // Check if client is valid
                    if client_option.is_none()
                        || client_option.unwrap().read().await.current_match.is_none()
                    {
                        let _ = send(&tx, "ERROR:INVALID:MOVE");
                        continue;
                    }

                    let client = client_option.unwrap().read().await;

                    let matches_guard = matches.read().await;
                    let current_match =
                        matches_guard.get(&client.current_match.unwrap()).unwrap().read().await;

                    let opponent = {
                        let result = if addr == current_match.player1 {
                            clients_guard.get(&current_match.player2).unwrap().read().await
                        } else {
                            clients_guard.get(&current_match.player1).unwrap().read().await
                        };

                        result
                    };

                    // Check if it's their move
                    if (current_match.ledger.is_empty() && current_match.first != addr)
                        || (current_match.ledger.last().is_some()
                            && current_match.ledger.last().unwrap().0 == client.color)
                    {
                        let _ = send(&tx, "ERROR:INVALID:MOVE");
                        continue;
                    }

                    let column_parse = text.split(":").collect::<Vec<&str>>()[1].parse::<usize>();

                    drop(current_match);
                    drop(matches_guard);

                    let mut matches_guard = matches.write().await;
                    let mut current_match = matches_guard
                        .get_mut(&client.current_match.unwrap())
                        .unwrap()
                        .write()
                        .await;

                    // Check if valid move
                    if let Ok(column) = column_parse {
                        if column >= 6 {
                            let _ = send(&tx, "ERROR:INVALID:MOVE");
                            continue;
                        }

                        if current_match.board[column][4] != Color::None {
                            let _ = send(&tx, "ERROR:INVALID:MOVE");
                            continue;
                        }

                        // Place it
                        current_match.place_token(client.color.clone(), column)
                    } else {
                        let _ = send(&tx, "ERROR:INVALID:MOVE");
                        continue;
                    }

                    // broadcast the move to viewers
                    broadcast_message(
                        &current_match.viewers,
                        &observers,
                        &format!("GAME:MOVE:{}:{}", client.username, column_parse.clone()?),
                    )
                    .await;

                    // Check game end conditions
                    let (winner, filled) = {
                        let mut result = (Color::None, false);

                        let mut any_empty = true;
                        for x in 0..6 {
                            for y in 0..5 {
                                let color = current_match.board[x][y].clone();
                                let mut horizontal_end = true;
                                let mut vertical_end = true;
                                let mut diagonal_end = true;

                                if any_empty && color == Color::None {
                                    any_empty = false;
                                }

                                for i in 0..4 {
                                    if x + i >= 6
                                        || current_match.board[x + i][y] != color && horizontal_end
                                    {
                                        horizontal_end = false;
                                    }

                                    if y + i >= 5
                                        || current_match.board[x][y + i] != color && vertical_end
                                    {
                                        vertical_end = false;
                                    }

                                    if x + i >= 6
                                        || y + i >= 5
                                        || current_match.board[x + i][y + i] != color
                                            && diagonal_end
                                    {
                                        diagonal_end = false;
                                    }
                                }

                                if horizontal_end || vertical_end || diagonal_end {
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
                    };

                    if winner != Color::None {
                        if winner == client.color {
                            let _ = send(&tx, "GAME:WINS");
                            if !demo_mode {
                                let _ = send(&opponent.connection, "GAME:LOSS");
                            }
                            broadcast_message(
                                &current_match.viewers,
                                &observers,
                                &format!("GAME:WIN:{}", client.username),
                            )
                            .await;
                        } else {
                            let _ = send(&tx, "GAME:LOSS");
                            if !demo_mode {
                                let _ = send(&opponent.connection, "GAME:WINS");
                            }
                            broadcast_message(
                                &current_match.viewers,
                                &observers,
                                &format!("GAME:WIN:{}", opponent.username),
                            )
                            .await;
                        }
                    } else if filled {
                        let _ = send(&tx, "GAME:DRAW");
                        if !demo_mode {
                            let _ = send(&opponent.connection, "GAME:DRAW");
                        }
                        broadcast_message(&current_match.viewers, &observers, "GAME:DRAW").await;
                    }

                    // remove match from matchmaker
                    if winner != Color::None || filled {
                        let opponent_addr = opponent.addr;
                        let current_match_id = current_match.id;

                        drop(client);
                        drop(opponent);
                        drop(current_match);
                        drop(clients_guard);

                        let mut clients_guard = clients.write().await;
                        let mut client = clients_guard.get_mut(&addr).unwrap().write().await;
                        client.current_match = None;
                        drop(client);

                        let mut opponent =
                            clients_guard.get_mut(&opponent_addr).unwrap().write().await;

                        if !demo_mode {
                            opponent.current_match = None;
                        }
                        matches_guard.remove(&current_match_id).unwrap();
                        continue;
                    }

                    if !demo_mode {
                        // TODO: delay/autoplay/continue behavior
                        let _ = send(
                            &opponent.connection,
                            &format!("OPPONENT:{}", column_parse.clone()?),
                        );
                    } else {
                        let random_move = random_move(&current_match.board);
                        current_match.place_token(Color::Blue, random_move);
                        let _ = send(&tx, &format!("OPPONENT:{}", random_move));
                    }
                }
				else if text == "GAME:LIST" {
                    todo!()
                }
				else if text.starts_with("GAME:WATCH:") {
                    todo!()
                }
				else if text.starts_with("ADMIN:AUTH:") {
                    todo!()
                }
				else if text.starts_with("ADMIN:KICK:") {
                    todo!()
                }
				else if text == "GAME:TERMINATE" {
                    todo!()
                }
				else if text.starts_with("GAME:AUTOPLAY:") {
                    todo!()
                }
				else if text == "GAME:CONTINUE" {
                    todo!()
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

    // Remove and terminate any matches
    if let Some(match_id) = clients.read().await.get(&addr).unwrap().read().await.current_match {
        let matches_guard = matches.read().await;
        let clients_guard = clients.read().await;
        let the_match = matches_guard.get(&match_id).unwrap().read().await;

        let player1 = clients_guard.get(&the_match.player1).unwrap().read().await;
        let player2 = clients_guard.get(&the_match.player2).unwrap().read().await;

        let _ = send(&player1.connection, "GAME:TERMINATED");
        let _ = send(&player2.connection, "GAME:TERMINATED");

        broadcast_message(&the_match.viewers, &observers, "GAME:TERMINATED").await;

        drop(player1);
        drop(player2);
        drop(the_match);
        drop(matches_guard);
        drop(clients_guard);

        matches.write().await.remove(&match_id);
    }

    clients.write().await.remove(&addr);
    observers.write().await.remove(&addr);

    let mut admin_guard = admin.write().await;
    if let Some(admin_addr) = *admin_guard {
        if admin_addr == addr {
            *admin_guard = None;
        }
    }
    drop(admin_guard);

    info!("Client {} removed", addr);

    Ok(())
}

async fn broadcast_message(addrs: &Vec<SocketAddr>, observers: &Observers, msg: &str) {
    for addr in addrs {
        let _ = send(observers.read().await.get(addr).unwrap(), msg);
    }
}

async fn watch(matches: &Matches, new_match_id: u32, addr: SocketAddr) {
    for a_match in &mut matches.write().await.values_mut() {
        let mut found = false;
        for i in 0..a_match.write().await.viewers.len() {
            if a_match.write().await.viewers[i] == addr {
                a_match.write().await.viewers.remove(i);
                found = true;
                break;
            }
        }

        if found {
            break;
        }
    }

    matches.write().await.get_mut(&new_match_id).unwrap().write().await.viewers.push(addr);
}

fn send(tx: &UnboundedSender<Message>, text: &str) -> Result<(), SendError<Message>> {
    tx.send(Message::text(text))
}
