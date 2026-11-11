---
type: concept
updated: 2026-11-11
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# El oráculo (recompensa sin tokens)

Lo que hace posible explorar sin modelo es que mcpOS detecta las fallas solo:

| señal | detección | firma para deduplicar |
|---|---|---|
| crash | la app muere por señal → coredump + gdb con [[toolkits/gdb-debuginfod]] | 5 funciones superiores de la pila |
| cuelgue | AT-SPI no responde en 15 s → `gdb -p` | pila de todos los hilos |
| exit ≠ 0 | estado de salida | código |
| log-error | `journalctl -p err _PID=…` | mensaje normalizado |

Regla aprendida con [[toolkits/firefox]]: una API que dice "éxito" no prueba que algo pasó; verificar el
efecto. Usado por [[concepts/go-explore]].
