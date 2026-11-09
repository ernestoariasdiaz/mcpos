[[ $- != *i* ]] && return
PS1='\[\e[1;36m\]mcpOS\[\e[0m\] \w \$ '
export PATH="$HOME/.local/bin:$PATH"
alias ll='ls -la'
alias audit='tail -f ~/.local/state/mcpd/audit.jsonl | jq -c "{tool,is_error,ms}"'
