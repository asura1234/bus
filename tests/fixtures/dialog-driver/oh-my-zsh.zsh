#!/bin/zsh -f
# Modeled oh-my-zsh prompt, exercised by a real zsh in a Bus shell pane.
# It performs no update and does not read or change the user's shell config.
printf '[oh-my-zsh] Would you like to update? [Y/n] '
read -r -k 1 answer
printf '\nOH_MY_ZSH_ANSWER:%s\n' "$answer"
[[ "$answer" == n ]]
