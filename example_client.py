import asyncio
import websockets
from enum import Enum

class Slot(Enum):
    NONE = 0
    RED = 1
    BLUE = 2

def calculate_move(opponent_move, board, our_color, opponent_color):
    if opponent_move is not None:
        print(f"Opponent played column {opponent_move}")
    # TODO: Implement your move calculation logic here instead
    # Use the board variable to see and set the current state of the board
    return int(input("Column: "))

async def gameloop (socket):
  board = [[Slot.NONE] * 5 for _ in range(6)]
  our_color = Slot.NONE
  opponent_color = Slot.NONE
  while True: # While game is active, continually anticipate messages
    message = (await socket.recv()).split(':') # Receive message from server
    print(f"Received message: {message}")

    match message[0]:
      case 'CONNECT':
        await socket.send('READY')

      case 'GAME':
        if message[1] == 'START':
            if message[2] == '1':
              our_color = Slot.RED
              opponent_color = Slot.BLUE
              col = calculate_move(None, board, our_color, opponent_color) # calculate_move is some arbitrary function you have created to figure out the next move
              await socket.send(f'PLAY:{col}') # Send your move to the sever
            else:
              our_color = Slot.BLUE
              opponent_color = Slot.RED

      case 'OPPONENT': # Opponent has gone; calculate next move
        col = calculate_move(message[1], board, our_color, opponent_color) # Give your function your opponent's move
        await socket.send(f'PLAY:{col}') #  Send your move to the sever

      case 'WIN' | 'LOSS' | 'DRAW' | 'TERMINATED': # Game has ended
        print(message[0])
        board = [[Slot.NONE] * 5 for _ in range(6)]
        our_color = None
        opponent_color = None
        await socket.send('READY')

      case 'KICK':
        print("You have been kicked from the game")
        break

      case 'ERROR':
        print(f"{message[0]}: {':'.join(message[1:])}")
        break

  await socket.close()

async def join_server(username):
  async with websockets.connect(f'wss://connect4.abunchofknowitalls.com') as socket: # Establish websocket connection
    await socket.send(f'CONNECT:{username}')
    await gameloop(socket)

if __name__ == '__main__': # Program entrypoint
    username = input("Enter username: ")
    asyncio.run(join_server(username))