---
type: toolkit
updated: 2026-11-11
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# Qt (apps de KDE)

- **Accesibilidad**: requiere `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` en la sesión (mcpOS lo exporta en `.bash_profile`).
- **"SetFocus" es ruido**: Qt expone la acción `SetFocus` en casi todos los controles. En [[apps/kcalc]]
  se llevó más de un tercio de las acciones de una corrida sin abrir nada nuevo. `mcp-explore` la ignora
  desde v0.5.3. (raw/notes/2026-11-09-11-sesiones-iniciales.md)
- **Qué abre cosas** en apps Qt: combos, casillas, pestañas y entradas de menú (50–64 % de novedad en
  KCalc); los `push button` casi nunca cambian la estructura, pero son donde viven los bugs de cálculo.
  Ver [[concepts/go-explore]].
- Ventanas/diálogos: se manejan con [[toolkits/kwin]].
