use connect4_moderator_server::{server::Server, *};
use futures_util::{SinkExt, StreamExt};
use local_ip_address::local_ip;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let demo_mode = args.get(1).is_some() && args.get(1).unwrap() == "demo";
    if demo_mode {
        info!("Starting server in DEMO MODE");
    }
    let admin_password = env::var("ADMIN_AUTH").unwrap_or_else(|_| String::from("admin"));
    info!("Admin password: {}", admin_password);
    let admin_password = Arc::new(admin_password);

    let addr = "0.0.0.0:8080";
    let listener = TcpListener::bind(&addr).await?;
    let hosted_addr = if local_ip().is_ok() {
        local_ip()?.to_string() + ":8080"
    } else {
        addr.to_string()
    };
    info!("WebSocket server listening on: {}", hosted_addr);

    let server_data = Arc::new(Server::new(admin_password.as_ref().clone(), demo_mode));

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
                                sd.handle_connect_cmd(addr, tx.clone(), requested_username).await
                            {
                                error!("handle_connect: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:ID:");
                        }
                    }
                    "RECONNECT" => {
                        if parts.len() > 1 {
                            let requested_username = parts[1].to_string();
                            if let Err(e) =
                                sd.handle_reconnect_cmd(addr, tx.clone(), requested_username).await
                            {
                                error!("handle_reconnect: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:RECONNECT:");
                        }
                    }
                    "DISCONNECT" => {
                        if let Err(e) = sd.handle_disconnect_cmd(addr, tx.clone()).await {
                            error!("handle_disconnect: {}", e);
                            let _ = send(&tx, e.to_string().as_str());
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
                        if parts.get(1) == Some(&"START") && parts.len() > 2 {
                            if let Err(e) =
                                sd.handle_tournament_start(addr, parts[2].to_string()).await
                            {
                                error!("handle_tournament_start: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"CANCEL") {
                            if let Err(e) = sd.handle_tournament_cancel(addr).await {
                                error!("handle_tournament_cancel: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:TOURNAMENT");
                        }
                    }
                    "GET" => {
                        if let Some(data_id) = parts.get(1) {
                            if let Err(e) =
                                sd.handle_get_data(tx.clone(), data_id.to_string()).await
                            {
                                error!("handle_get_data: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:GET");
                        }
                    }
                    "SET" => {
                        if parts.len() > 2 {
                            let data_id = parts[1].to_string();
                            let data_value = parts[2].to_string();
                            if let Err(e) =
                                sd.handle_set_data(tx.clone(), addr, data_id, data_value).await
                            {
                                error!("handle_set_data: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:SET");
                        }
                    }
                    "RESERVATION" => {
                        if parts.get(1) == Some(&"ADD") && parts.len() == 3 {
                            let usernames = parts[2].split(",").collect::<Vec<&str>>();
                            if usernames.len() != 2 {
                                error!("handle_reservation_add: invalid number of usernames");
                                let _ = send(&tx, "ERROR:INVALID:RESERVATION");
                                continue;
                            }

                            let player1_username = usernames[0].to_string();
                            let player2_username = usernames[1].to_string();

                            if let Err(e) = sd.handle_reservation_add(tx.clone(), addr, player1_username, player2_username).await {
                                error!("handle_reservation_add: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"DELETE") && parts.len() == 3 {
                            let usernames = parts[2].split(",").collect::<Vec<&str>>();
                            if usernames.len() != 2 {
                                error!("handle_reservation_delete: invalid number of usernames");
                                let _ = send(&tx, "ERROR:INVALID:RESERVATION");
                                continue;
                            }

                            let player1_username = usernames[0].to_string();
                            let player2_username = usernames[1].to_string();

                            if let Err(e) = sd.handle_reservation_delete(tx.clone(), addr, player1_username, player2_username).await {
                                error!("handle_reservation_delete: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else if parts.get(1) == Some(&"GET") {
                            if let Err(e) = sd.handle_reservation_get(tx.clone(), addr).await {
                                error!("handle_reservation_get: {}", e);
                                let _ = send(&tx, e.to_string().as_str());
                            }
                        } else {
                            let _ = send(&tx, "ERROR:INVALID:RESERVATION");
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
        let tournament_guard = sd.tournament.read().await;
        if client.current_match.is_some() {
            sd.disconnected_clients.write().await.push(username.clone());
        } else if tournament_guard.is_some() {
            let tourney = tournament_guard.clone().unwrap();
            if tourney.read().await.contains_player(addr) {
                sd.disconnected_clients.write().await.push(username.clone());
            }
        }

        drop(client);
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
