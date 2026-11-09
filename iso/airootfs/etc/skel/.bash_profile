[[ -f ~/.bashrc ]] && . ~/.bashrc
if [[ -z $WAYLAND_DISPLAY && $(tty) == /dev/tty1 ]]; then
    export WLR_NO_HARDWARE_CURSORS=1
    # no 3D in the VM? fall back to software rendering
    [[ -e /dev/dri/renderD128 ]] || export WLR_RENDERER=pixman
    export XDG_CURRENT_DESKTOP=sway MOZ_ENABLE_WAYLAND=1
    exec sway > ~/.sway.log 2>&1
fi
