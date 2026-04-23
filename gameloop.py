import asyncio
import websockets
from websockets.exceptions import ConnectionClosed
from wakepy import keep
from agent import Agent

DEFAULT_SERVER_URL = "wss://connect4.abunchofknowitalls.com/ws"
RECONNECT_INTERVAL_SECONDS = 5
RECONNECT_TIMEOUT_SECONDS = 60
MAX_RECONNECT_ATTEMPTS = (
  (RECONNECT_TIMEOUT_SECONDS + RECONNECT_INTERVAL_SECONDS - 1)
  // RECONNECT_INTERVAL_SECONDS
)

async def gameloop(socket):
  player = Agent()

  while True:  # Receive messages until the connection closes
    message = (await socket.recv()).split(":")  # Receive message from server

    match message[0]:
      case "CONNECT":
        await socket.send("READY")
      case "RECONNECT":
        await socket.send("READY")

      case "GAME":
        if message[1] == "START":
          if message[2] == "1":
            # calculate_move is some arbitrary function you have created to figure out the next move
            col = player.calculate_move(None)
            await socket.send(f"PLAY:{col}")
        if (message[1] == "WINS") | (message[1] == "LOSS") | (message[1] == "DRAW") | (message[1] == "TERMINATED"):
          print(message[0] + ":" + message[1])
          player.reset()

      case "OPPONENT":
        # Opponent has gone; calculate next move
        col = player.calculate_move(message[1])
        await socket.send(f"PLAY:{col}")  # Send your move to the sever
        
      case "TOURNAMENT":
        if message[1] == "END":
          await socket.send("READY")

      case "ERROR":
        print(f"{message[0]}: {':'.join(message[1:])}")

async def join_server(username, server_url):
  reconnecting = False
  reconnect_deadline = None
  reconnect_attempt = 0

  while True:
    try:
      async with websockets.connect(server_url, ping_interval=30, ping_timeout=30) as socket:
        if reconnecting:
          await socket.send(f"RECONNECT:{username}")
          print("Reconnected to server.")
        else:
          await socket.send(f"CONNECT:{username}")

        reconnect_deadline = None
        reconnect_attempt = 0
        await gameloop(socket)
    except (ConnectionClosed, OSError) as error:
      print(f"Connection lost ({error}).")
    else:
      print("Connection closed.")

    now = asyncio.get_running_loop().time()
    if reconnect_deadline is None:
      reconnect_deadline = now + RECONNECT_TIMEOUT_SECONDS
      print(f"Attempting to reconnect every {RECONNECT_INTERVAL_SECONDS} seconds for up to {RECONNECT_TIMEOUT_SECONDS} seconds...")

    remaining = reconnect_deadline - now
    if remaining <= 0:
      print("Failed to reconnect within 60 seconds. Exiting.")
      return

    reconnecting = True
    reconnect_attempt += 1
    wait_time = min(RECONNECT_INTERVAL_SECONDS, remaining)
    print(
      f"Reconnect attempt {reconnect_attempt}/{MAX_RECONNECT_ATTEMPTS}: "
      f"retrying in {wait_time:.0f}s "
      f"({remaining:.0f}s remaining)."
    )
    await asyncio.sleep(wait_time)


if __name__ == "__main__":
  with keep.presenting():
    server_url = (
      input(f"Enter server address [{DEFAULT_SERVER_URL}]: ").strip()
      or DEFAULT_SERVER_URL
    )
    username = input("Enter username: ")
    asyncio.run(join_server(username, server_url))
