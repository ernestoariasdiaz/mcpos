---
type: toolkit
updated: 2026-11-09
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# KWin (KDE Plasma 6, Wayland)

| Necesidad | Lo que funciona | Lo que no |
|---|---|---|
| Teclado / ratón | `ydotool` vía `/dev/uinput` (servicio `mcpos-ydotoold`) | `wtype` (KWin no ofrece teclado virtual) |
| Clic por coordenadas | aceleración plana en el dispositivo de ydotool (`org.kde.KWin.InputDevice`) + verificar contra `workspace.cursorPos` y corregir | `ydotool mousemove --absolute` a secas: se desvía por la aceleración |
| Ventanas / escritorios | scripts de KWin cargados por D-Bus (`org.kde.kwin.Scripting`) que responden por un servicio D-Bus propio | — |
| Capturas | `spectacle -b -n -f -o FILE` | `grim` (no hay wlr-screencopy) |

- Ejemplo verificado: clic en "File" de Kate en (137,644) "after 1 correction(s)"; antes caía en la barra
  de título. (raw/notes/2026-11-09-11-sesiones-iniciales.md)
- **Spectacle es de instancia única**: si el humano está capturando, la captura del agente espera.
  `desktop__screenshot` ahora falla al instante con un mensaje claro y nunca cierra la del humano.
- Para capturar una ventana tapada: `desktop__screenshot(window="…")` la fija encima y recorta.
- Pegar en Konsole: Ctrl+Shift+V.
