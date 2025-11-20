import asyncio

import websockets

async def calculate_move(opponent_move, board):
  if opponent_move is not None:
    print(f"Opponent played column {opponent_move}")
  # TODO: Implement your move calculation logic here instead
  # Use the board variable to see and set the current state of the board
  loop = asyncio.get_running_loop()
  return int(await loop.run_in_executor(None, input, "Column: "))


async def gameloop(socket):
  board = [[None] * 6 for _ in range(7)]
  while True:  # While game is active, continually anticipate messages
    message = (await socket.recv()).split(':')  # Receive message from server

    match message[0]:
      case 'CONNECT':
        await socket.send('READY')

      case 'GAME':
        if message[1] == 'START':
          if message[2] == '1':
            col = await calculate_move(None, board)  # calculate_move is some arbitrary function you have created to figure out the next move
            await socket.send(f'PLAY:{col}')  # Send your move to the sever
        if (message[1] == 'WINS') | (message[1] == 'LOSS') | (message[1] == 'DRAW') | (message[1] == 'TERMINATED'): # Game has ended
          print(message[0]+":"+message[1])
          board = [[None] * 6 for _ in range(7)]
          await socket.send('READY')

      case 'OPPONENT':  # Opponent has gone; calculate next move
        col = await calculate_move(message[1], board)  # Give your function your opponent's move
        await socket.send(f'PLAY:{col}')  # Send your move to the sever

      case 'ERROR':
        print(f"{message[0]}: {':'.join(message[1:])}")
  
  await socket.close()

async def join_server(username):
  async with websockets.connect(f'wss://connect4.abunchofknowitalls.com', ping_interval=30, ping_timeout=30) as socket: # Establish websocket connection
    await socket.send(f'CONNECT:{username}')
    await gameloop(socket)


if __name__ == '__main__':  # Program entrypoint
  username = input("Enter username: ")
  asyncio.run(join_server(username))
