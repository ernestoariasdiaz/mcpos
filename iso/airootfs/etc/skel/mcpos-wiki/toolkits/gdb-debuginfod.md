---
type: toolkit
updated: 2026-11-11
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# gdb + debuginfod (Arch)

- `journal__coredump_backtrace` y `mcp-explore` usan `DEBUGINFOD_URLS=https://debuginfod.archlinux.org`
  y `gdb -iex 'set debuginfod enabled on'`: los símbolos se descargan solos, sin paquetes `-debug`.
- La primera vez para una app grande puede tardar minutos (descarga).
- Verificado con bash: `make_child (jobs.c:2365)`, `restore_sigmask (sig.c:499)`.
  (raw/notes/2026-11-09-11-sesiones-iniciales.md)
- Pista de lectura: `si_code: SI_USER` = la señal la mandó otro proceso (`kill`), no un fallo de memoria.
