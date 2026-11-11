---
type: concept
updated: 2026-11-11
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# Estados fantasma

Si el "estado" de la interfaz incluyera relojes, contadores o el texto del display, cada segundo parecería
un estado nuevo y la curiosidad del explorador se inflaría con duplicados. Es el mismo modo de fallo que
los *Phantom Nodes* de ANIMUS (52 % de nodos duplicados no detectados).

`mcp-explore` hashea solo la estructura visible: rol, nombre normalizado (dígitos → `#`) y banderas
marcado/seleccionado/expandido (para que un cambio de modo sí cuente). El texto queda fuera.

Lo mismo vale para esta wiki: dos páginas del mismo concepto con nombres distintos son "páginas fantasma".
`mcpos-wiki-lint` avisa de nombres casi iguales; fusionarlas es parte del lint.
Relacionado: [[concepts/go-explore]].
