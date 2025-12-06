# Connect4 Moderator - Server

A WebSocket server for the RPI Minds & Machines class and the Connect4 Final Project.

# To Build Your AI
Download the [`gameloop.py`](https://github.com/joshuafhiggins/connect4-moderator-server/blob/main/gameloop.py) and [`agent.py`](https://github.com/joshuafhiggins/connect4-moderator-server/blob/main/agent.py) files. It is heavily encouraged you only make changes to the `agent.py` file

In order to run your AI, you'll need:
- Python 3
- `pip install websockets` (Windows) or `pip3 install websockets` (Linux/macOS)
- `pip install pip-system-certs` (Windows) or `pip3 install pip-system-certs` (Linux/macOS)

To run the example, run `python gameloop.py`.

# To Watch Games
The visual client for observing games/tournaments as well as managing games can be found at [`connect4-moderator-observer`](https://github.com/joshuafhiggins/connect4-moderator-observer).

# To Run the Server Locally
- Install Git and the Rust programming language
- `git clone https://github.com/joshuafhiggins/connect4-moderator-server.git`
- `cd connect4-moderator-server`
- `cargo run --release demo`

In this mode, you'll play against a player named `demo` who makes moves at random. If your AI makes invalid moves then your match is terminated and kicked from the server. If during the tournament you make an invalid move, you will instead immediately lose your game.

To connect to this server, use the address `ws://localhost:8080`

# For Future Maintainers:
The Google Sheet outlining the communication protocol can be found [here](https://docs.google.com/spreadsheets/d/1qPrNvB4-1jzvkaQXJcA1Z2OllpSQYmtMRfbOgSi4Yhw), for those of you who'd prefer to not write their AI in python

A JavaScript debug client that text raw text as input and prints responses is provided,
- `npm i`
- `node debug_client.js`

I also apologize in advance.