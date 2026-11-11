---
type: note
source: sesiones de construcción y prueba de mcpOS (Ernesto + Claude), 9–11 nov 2026, VMware Workstation en Windows 11
---

# Lo verificado en las primeras sesiones de mcpOS

Hechos observados en la VM real (no en mocks), en orden.

## Firefox (Gecko) y AT-SPI — 9 nov
- La barra de direcciones aparece como `combo box` con un campo editable dentro.
- `EditableText.set_text_contents` sobre ella devolvía `true` y la barra seguía vacía.
- Funciona: buscar el hijo editable, leer el texto de vuelta, y si no quedó, foco + Ctrl+A + teclear.
  Se implementó así en `a11y__set_text` (v0.3.1).
- `a11y__list_apps`, `tree`, `find` y `wait_for` funcionaron con Firefox.

## KDE Plasma 6 / KWin (Wayland) — 9 nov
- `wtype` no sirve en KWin (no expone el protocolo de teclado virtual): teclado y ratón van por `ydotool` (uinput).
- Ventanas y escritorios solo por scripts de KWin cargados vía D-Bus (`org.kde.kwin.Scripting`); funcionó
  get_tree, launch, activate y close con Dolphin y Kate.
- El clic absoluto con `ydotool` caía en la barra de título del menú "File" de Kate (aceleración del puntero).
  Con aceleración plana vía `org.kde.KWin.InputDevice` + verificación contra `workspace.cursorPos`:
  "verified at 137,644 after 1 correction(s)" y el menú se abrió (v0.4.2).
- `spectacle -b -n -f -o FILE` sirve para capturas sin ventana. Spectacle es de instancia única: si el
  usuario tiene abierta su selección de región, la captura del agente espera hasta agotar el tiempo.
- Konsole se reponía delante de otras ventanas al capturar; capturar con la ventana en "keep above" y
  recortar a su geometría lo resolvió (`desktop__screenshot(window=...)`).
- Pegar en Konsole es Ctrl+Shift+V. El portapapeles VMware↔VM por XWayland/vmware-user fue poco fiable.

## Depuración — 9 y 11 nov
- `systemd-coredump` + `coredumpctl debug` con gdb funcionó; con `DEBUGINFOD_URLS=https://debuginfod.archlinux.org`
  y `-iex 'set debuginfod enabled on'` gdb bajó los símbolos de bash: pila con `make_child (jobs.c:2365)`,
  `restore_sigmask (sig.c:499)`.
- `bash -c 'sleep 300 & kill -SEGV $!'`: la señal llega al hijo entre fork y exec (`pid = 0` en `make_child`,
  `si_code: SI_USER`).

## mcpd / actualización — 10 nov
- `mcpd` sigue usando los binarios viejos de sus servidores tras actualizar hasta que se reinician;
  `mcpos-update` los reinicia (menos el que lo ejecuta y las corridas `mcp-explore run`).

## KCalc con mcp-explore — 11 nov
- Primer intento: KCalc no estaba instalado (salida 127). Se añadió a la ISO.
- Corrida v0.5.1 (UCB, "sin probar = infinito"): 30 min, 3201 acciones, 65 reinicios, 14 estados, 0 hallazgos.
- Corrida v0.5.2 (Go-Explore + prioridades por tipo de control): 30 min, 3205 acciones, 37 reinicios,
  88 estados, 0 hallazgos.
  - Exploró: menús File, Edit, Settings, Help, Constants (Mathematics, Electromagnetism, Atomic & Nuclear);
    bases Dec/Hex/Oct/Bin y la casilla Shift; diálogos Configure Keyboard Shortcuts, About KDE, About KCalc,
    Show History, Report Bug.
  - ~1200 acciones (más de un tercio) fueron solo "SetFocus" sobre botones, paneles o campos: ninguna
    llevó a un estado nuevo.
  - Combos, casillas, pestañas y entradas de menú descubrieron algo nuevo en el 50–64 % de sus usos,
    con solo 7–33 intentos cada uno.
  - Sin probar 83 de 215 acciones: x!, xʸ, x^(1/y), raíces, nCm, mod; AND, OR, XOR, Lsh, Rsh, Cmp, IntDiv;
    memoria (MS, M+, M−…); botones de diálogos (OK, Cancel, Defaults, Close, Print, Manage Schemes,
    Launch Bug Report Wizard).
- Respuesta (v0.5.3): ignorar acciones de foco y bono por acción nunca probada. En un mock de 43 pantallas
  y 553 acciones con el crash tras un botón que no cambia la pantalla: v0.5.2 lo halló en 1/3 semillas,
  v0.5.3 en 3/3, probando 93–96 % de las acciones. **Pendiente: repetir en KCalc real.**

## Kate
- Kate es de instancia única en KDE; para explorarla con un HOME aparte se espera necesitar `kate -n`.
  **No verificado aún en mcpOS.**
