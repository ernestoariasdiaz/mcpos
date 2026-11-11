---
type: toolkit
updated: 2026-11-09
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# Firefox (Gecko)

- La barra de direcciones es un `combo box` con un campo editable hijo.
- **`set_text_contents` miente**: devuelve `true` sin cambiar la barra. Por eso `a11y__set_text` busca el
  hijo editable, **verifica leyendo el texto** y si no quedó usa foco + Ctrl+A + teclado.
  (raw/notes/2026-11-09-11-sesiones-iniciales.md)
- `list_apps`, `tree`, `find`, `wait_for` funcionan bien con Firefox.
- Lección general: con AT-SPI, "éxito" no significa que pasó algo. Verificar siempre el efecto
  ([[concepts/oraculo]]).
