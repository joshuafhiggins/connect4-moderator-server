import asyncio
import itertools

import websockets
from enum import Enum

from websockets import ConnectionClosed


def calculate_move(opponent_move, board):
  if opponent_move is not None:
    print(f"Opponent played column {opponent_move}")
  # TODO: Implement your move calculation logic here instead
  # Use the board variable to see and set the current state of the board
  return int(input("Column: "))


async def gameloop(socket):
  board = [[None] * 5 for _ in range(6)]
  while True:  # While game is active, continually anticipate messages
    message = (await socket.recv()).split(':')  # Receive message from server

    match message[0]:
      case 'CONNECT':
        await socket.send('READY')

      case 'GAME':
        if message[1] == 'START':
          if message[2] == '1':
            col = calculate_move(None, board)  # calculate_move is some arbitrary function you have created to figure out the next move
            await socket.send(f'PLAY:{col}')  # Send your move to the sever
        if (message[1] == 'WINS') | (message[1] == 'LOSS') | (message[1] == 'DRAW') | (message[1] == 'TERMINATED'): # Game has ended
          print(message[0]+message[1])
          board = [[None] * 5 for _ in range(6)]
          our_color = None
          opponent_color = None
          await socket.send('READY')

      case 'OPPONENT':  # Opponent has gone; calculate next move
        col = calculate_move(message[1], board)  # Give your function your opponent's move
        await socket.send(f'PLAY:{col}')  # Send your move to the sever

      case 'ERROR':
        print(f"{message[0]}: {':'.join(message[1:])}")
        break

  await socket.close()

async def keepalive(websocket, ping_interval=30):
  for ping in itertools.count():
    await asyncio.sleep(ping_interval)
    try:
      await websocket.send("PING")
    except ConnectionClosed:
      break

async def join_server(username):
  async with websockets.connect(f'wss://connect4.abunchofknowitalls.com') as socket: # Establish websocket connection
    keepalive_task = asyncio.create_task(keepalive(socket))
    try:
      await socket.send(f'CONNECT:{username}')
      await gameloop(socket)
    finally:
      keepalive_task.cancel()


if __name__ == '__main__':  # Program entrypoint
  username = input("Enter username: ")
  asyncio.run(join_server(username))
