[[ -f ~/.bashrc ]] && . ~/.bashrc
if [[ -z $WAYLAND_DISPLAY && $(tty) == /dev/tty1 ]]; then
    export WLR_NO_HARDWARE_CURSORS=1
    # no 3D in the VM? fall back to software rendering
    [[ -e /dev/dri/renderD128 ]] || export WLR_RENDERER=pixman
    export XDG_CURRENT_DESKTOP=sway MOZ_ENABLE_WAYLAND=1
    # accessibility on for every toolkit, so mcp-a11y can read all apps
    export GNOME_ACCESSIBILITY=1 QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1 QT_ACCESSIBILITY=1 NO_AT_BRIDGE=0
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
    exec sway > ~/.sway.log 2>&1
fi
