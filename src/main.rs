mod types;

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use tracing::{info, error, warn};
use types::Client;
use crate::types::{Color, Role, AI};

type Clients = Arc<RwLock<HashMap<SocketAddr, Client>>>;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
	// Initialize logging
	tracing_subscriber::fmt::init();

	let addr = "127.0.0.1:8080";
	let listener = TcpListener::bind(&addr).await?;
	info!("WebSocket server listening on: {}", addr);

	let mut clients: Clients = Arc::new(RwLock::new(HashMap::new()));

	while let Ok((stream, addr)) = listener.accept().await {
		tokio::spawn(handle_connection(stream, addr, clients.clone()));
	}

	Ok(())
}

async fn handle_connection(
	stream: TcpStream,
	addr: SocketAddr,
	clients: Clients,
) -> Result<(), anyhow::Error> {
	info!("New WebSocket connection from: {}", addr);

	let ws_stream = accept_async(stream).await?;
	let (mut ws_sender, mut ws_receiver) = ws_stream.split();
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

	// Store the client
	{
	  clients.write().await.insert(addr, Client::new(String::new(), Role::Observer, tx));
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
								is_taken = true;
								break;
							}
						}
					}

					if is_taken {
						let _ = clients.read().await.get(&addr).unwrap().send("ERROR:INVALID:ID:".to_string() + &*requested_username);
						continue;
					}

					// not taken
					clients.write().await.get_mut(&addr).unwrap().role = Role::Player;
					clients.write().await.get_mut(&addr).unwrap().username = Some(requested_username);
				}
				else if text.starts_with("READY") {
					// TODO!
				}
				else if text.starts_with("PLAY") {
					// TODO!
				}
				else {
					let _ = clients.read().await.get(&addr).unwrap().send("ERROR:UNKNOWN".to_string());
				}
			}
			Ok(Message::Close(_)) => {
				info!("Client {} disconnected", addr);
				break;
			}
			Ok(_) => { let _ = clients.read().await.get(&addr).unwrap().send("ERROR:UNKNOWN".to_string()); }
			Err(e) => {
				error!("WebSocket error for {}: {}", addr, e);
				break;
			}
		}
	}

	// Clean up
	send_task.abort();
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