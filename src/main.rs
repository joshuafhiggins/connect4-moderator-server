use connect4_moderator_server::{server::Server, *};
use futures_util::{SinkExt, StreamExt};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{error, info};

// TODO: Allow random "player1" in demo mode
// TODO: Support reconnecting behaviors
// TODO: Other tournament types
// TODO: Max move wait time
// TODO: Show tournament scoreboard after every round of games
// TODO: Tiebreakers, guarantee some amount of going first
// TODO: Send moves instantly, sleep only till waiting time

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let demo_mode = args.get(1).is_some() && args.get(1).unwrap() == "demo";
    let tournament_type = if !demo_mode {
        if let Some(tourney) = args.get(1) {
            tourney.clone()
        } else {
            "round_robin".to_string()
        }
    } else {
        "round_robin".to_string()
    };
    let admin_password = env::var("ADMIN_AUTH").unwrap_or_else(|_| String::from("admin"));
    info!("Admin password: {}", admin_password);
    let admin_password = Arc::new(admin_password);

    let addr = "0.0.0.0:8080";
    let listener = TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on: {}", addr);

    let server_data = Arc::new(Server::new(
        admin_password.as_ref().clone(),
        demo_mode,
        tournament_type,
    ));

    while let Ok((stream, addr)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, addr, server_data.clone()));
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
                let parts: Vec<&str> = text.split(':').collect();
                let cmd = parts[0];
                match cmd {
                    "CONNECT" => {
                        if parts.len() > 1 {
                            let requested_username = parts[1].to_string();
                            if let Err(e) =
                                sd.handle_connect(addr, tx.clone(), requested_username).await
                            {
                                error!("handle_connect: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:ID:");
                        }
                    }
                    "READY" => {
                        if let Err(e) = sd.handle_ready(addr, tx.clone()).await {
                            error!("handle_ready: {}", e);
                            let _ = send(&tx, e.to_string().as_str());
                        }
                    }
                    "PLAY" => {
                        if parts.len() > 1 {
                            match parts[1].parse::<usize>() {
                                Ok(column) => {
                                    if let Err(e) = sd.handle_play(addr, tx.clone(), column).await {
                                        error!("handle_play: {}", e);
                                        let _ = send(&tx, e.to_string().as_str());
                                    }
                                }
                                Err(_) => {
                                    let _ = send(&tx, "ERROR:INVALID:MOVE");
                                }
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:MOVE");
                        }
                    }
                    "PLAYER" => {
                        if parts.get(1) == Some(&"LIST") {
                            if let Err(e) = sd.handle_player_list(tx.clone()).await {
                                error!("handle_player_list: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:PLAYER");
                        }
                    }
                    "GAME" => {
                        if parts.get(1) == Some(&"LIST") {
                            if let Err(e) = sd.handle_game_list(tx.clone()).await {
                                error!("handle_game_list: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"WATCH") && parts.len() > 2 {
                            match parts[2].parse::<u32>() {
                                Ok(match_id) => {
                                    if let Err(e) =
                                        sd.handle_game_watch(tx.clone(), match_id, addr).await
                                    {
                                        error!("handle_game_watch: {}", e);
                                        let _ = send(&tx, e.to_string().as_str());
                                    }
                                }
                                Err(_) => {
                                    let _ = send(&tx, "ERROR:INVALID:WATCH");
                                }
                            }
                        } else if parts.get(1) == Some(&"TERMINATE") && parts.len() > 2 {
                            match parts[2].parse::<u32>() {
                                Ok(match_id) => {
                                    if let Err(e) = sd.handle_game_terminate(addr, match_id).await {
                                        error!("handle_game_terminate: {}", e);
                                        let _ = send(&tx, e.to_string().as_str());
                                    }
                                }
                                Err(_) => {
                                    let _ = send(&tx, "ERROR:INVALID:TERMINATE");
                                }
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:GAME");
                        }
                    }
                    "ADMIN" => {
                        if parts.get(1) == Some(&"AUTH") && parts.len() > 2 {
                            if let Err(e) =
                                sd.handle_admin_auth(tx.clone(), addr, parts[2].to_string()).await
                            {
                                error!("handle_admin_auth: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"KICK") && parts.len() > 2 {
                            if let Err(e) = sd.handle_admin_kick(addr, parts[2].to_string()).await {
                                error!("handle_admin_kick: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:ADMIN");
                        }
                    }
                    "TOURNAMENT" => {
                        if parts.get(1) == Some(&"START") {
                            if let Err(e) = sd.handle_tournament_start(tx.clone(), addr).await {
                                error!("handle_tournament_start: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"CANCEL") {
                            if let Err(e) = sd.handle_tournament_cancel(tx.clone(), addr).await {
                                error!("handle_tournament_cancel: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"WAIT") && parts.len() > 2 {
                            match parts[2].parse::<f64>() {
                                Ok(new_timeout) => {
                                    if let Err(e) =
                                        sd.handle_tournament_wait(addr, new_timeout).await
                                    {
                                        error!("handle_tournament_wait: {}", e);
                                        let _ = send(&tx, e.to_string().as_str());
                                    }
                                }
                                Err(_) => {
                                    let _ = send(&tx, "ERROR:INVALID:TOURNAMENT");
                                }
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:TOURNAMENT");
                        }
                    }
                    "GET" => {
                        if parts.get(1) == Some(&"MOVE_WAIT") {
                            if let Err(e) = sd.handle_get_move_wait(tx.clone()).await {
                                error!("handle_get_move_wait: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"TOURNAMENT_STATUS") {
                            if let Err(e) = sd.handle_get_tournament_status(tx.clone()).await {
                                error!("handle_get_tournament_status: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:GET");
                        }
                    }
                    _ => {
                        let _ = send(&tx, "ERROR:UNKNOWN");
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Client {} disconnected", addr);
                break;
            }
            Ok(Message::Binary(_)) => {
                let _ = send(&tx, "ERROR:UNKNOWN");
            }
            Ok(_) => {} // Ping packets, we can ignore, they get handled for us
            Err(e) => {
                error!("WebSocket error for {}: {}", addr, e);
                break;
            }
        }
    }

    // Clean up
    send_task.abort();

    // Remove and terminate any matches
    // We may not be a client disconnecting, do this check
    let clients_guard = sd.clients.read().await;
    if clients_guard.get(&addr).is_some() {
        let client = clients_guard.get(&addr).unwrap().read().await;
        let username = client.username.clone();
        if let Some(match_id) = client.current_match {
            drop(client);
            sd.terminate_match(match_id).await;
        } else {
            drop(client);
        }

        drop(clients_guard);

        sd.clients.write().await.remove(&addr);
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
