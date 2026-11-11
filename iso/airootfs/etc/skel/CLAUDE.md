# mcpOS — contexto para el agente

Estás en **mcpOS**, una VM de pruebas (Arch + KDE Plasma 6, Wayland). Tienes el sistema entero a través
del servidor MCP `mcpd`: `shell__*` (root permitido), `journal__*` (logs, coredumps, gdb + debuginfod),
`desktop__*` (KWin, screenshots, teclado/ratón), `a11y__*` (árbol de accesibilidad) y `explore__*`
(exploración de apps sin tokens). Es una VM desechable: puedes romper cosas, pero no borres los datos
del usuario en `~` fuera de lo que te pidan.

## Memoria: `~/mcpos-wiki`

Antes de explorar, depurar o automatizar una app, **lee `~/mcpos-wiki/index.md`** y la página de esa
app o de su toolkit: ahí está lo ya aprendido (cómo lanzarla, rarezas de Qt/GTK/Firefox/KWin, bugs
conocidos, rutas semilla). Mantener esa wiki es parte de tu trabajo; sus reglas están en
`~/mcpos-wiki/CLAUDE.md`. En particular:

- Después de cada `explore__start` que termine: `explore__digest` y luego **ingesta** según el esquema.
- Si descubres algo duradero sobre una app o un toolkit (aunque no venga de una exploración), anótalo
  en `~/mcpos-wiki/raw/notes/` y compílalo en la página correspondiente.
- Cuando el humano diga "lint de la wiki": `mcpos-wiki-lint` y la revisión manual del esquema.
