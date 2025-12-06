from typing import Union

class Agent:

  def calculate_move(self, col: Union[int, None]) -> int:
    if col is None:
      # TODO: Determine what move to make when going first
      raise NotImplementedError("Agent needs to implement calculate_move")

    # TODO: Determine what move to make, given the opponent just played in the 'col' index of the board
    raise NotImplementedError("Agent needs to implement calculate_move")

  def reset(self):
    # TODO: Reset the Agent's internal states for a new game
    raise NotImplementedError("Agent needs to implement reset()")