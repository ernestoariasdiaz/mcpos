---
type: app
app: kate
toolkit: qt
runs: 0
bugs: 0
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# Kate

## Resumen
Editor de KDE ([[toolkits/qt]]). Usado para probar `desktop__*`: escribir con `ydotool`, Ctrl+A/Ctrl+C,
clic verificado en "File" (ver [[toolkits/kwin]]). Aún no explorado con `mcp-explore`.

## Cómo lanzarla
Kate es de instancia única: con otra Kate abierta, `kate` a secas se pasaría a esa ventana. Para explorar
se usa `kate -n` (nueva instancia). **Hipótesis, no verificado en mcpOS.**

## Pendiente
- Primera corrida: `explore__start(app="kate")` (este bloque aplica `kate -n`), 30 min.

```mcpos-explore
{"launch": "kate -n", "deny": [], "seeds": []}
```
