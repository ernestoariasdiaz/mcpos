[[ -f ~/.bashrc ]] && . ~/.bashrc
if [[ -z $WAYLAND_DISPLAY && $(tty) == /dev/tty1 ]]; then
    # accessibility on for every toolkit, so mcp-a11y can read all apps
    export GNOME_ACCESSIBILITY=1 QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1 QT_ACCESSIBILITY=1 NO_AT_BRIDGE=0
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
    export MOZ_ENABLE_WAYLAND=1
    # no 3D in the VM? software rendering
    [[ -e /dev/dri/renderD128 ]] || export KWIN_COMPOSE=Q WLR_RENDERER=pixman

    session=$(cat ~/.config/mcpos/session 2>/dev/null || echo plasma)
    case $session in
        sway)
            export XDG_CURRENT_DESKTOP=sway WLR_NO_HARDWARE_CURSORS=1
            exec sway > ~/.sway.log 2>&1 ;;
        *)
            exec /usr/lib/plasma-dbus-run-session-if-needed /usr/bin/startplasma-wayland > ~/.plasma.log 2>&1 ;;
    esac
fi
