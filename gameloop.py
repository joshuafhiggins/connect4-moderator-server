import asyncio
import websockets

DEFAULT_SERVER_URL = "wss://connect4.abunchofknowitalls.com"

from agent import Agent

async def gameloop(socket):
  player = Agent()

  while True:  # While game is active, continually anticipate messages
    message = (await socket.recv()).split(":")  # Receive message from server

    match message[0]:
      case "CONNECT":
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
          await socket.send("READY")

      case "OPPONENT":
        # Opponent has gone; calculate next move
        col = player.calculate_move(message[1])
        await socket.send(f"PLAY:{col}")  # Send your move to the sever

      case "ERROR":
        print(f"{message[0]}: {':'.join(message[1:])}")

  await socket.close()


async def join_server(username, server_url):
  async with websockets.connect(server_url, ping_interval=30, ping_timeout=30) as socket:
    await socket.send(f"CONNECT:{username}")
    await gameloop(socket)


if __name__ == "__main__":
  server_url = (
    input(f"Enter server address [{DEFAULT_SERVER_URL}]: ").strip()
    or DEFAULT_SERVER_URL
  )
  username = input("Enter username: ")
  asyncio.run(join_server(username, server_url))
