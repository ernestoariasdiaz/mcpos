# mcpOS wiki — esquema

Esta carpeta es la memoria compilada de mcpOS: lo que se ha aprendido explorando y depurando apps
del escritorio. Sigue el patrón *LLM Wiki* de Karpathy: **tú (el agente) escribes y mantienes la wiki;
el humano elige qué explorar y hace preguntas.** El objetivo es no redescubrir nada: cada corrida,
cada bug y cada rareza de un toolkit se compila una vez y se reutiliza.

## Capas

| Capa | Ruta | Reglas |
|---|---|---|
| Fuentes en bruto | `raw/runs/*.md` (digests de `explore__digest`), `raw/notes/*.md` (notas del humano), `raw/upstream/*.md` (reportes de bugs enviados) | **Inmutables.** Se leen, nunca se editan ni se borran. |
| Wiki | `apps/`, `toolkits/`, `bugs/`, `concepts/`, `overview.md` | Tuya. Créala, reescríbela y reorganízala. |
| Índices | `index.md` (catálogo), `log.md` (cronológico, solo se añade) | Se actualizan en **cada** operación. |
| Esquema | este archivo | Se cambia junto con el humano cuando una regla no funciona. |

## Tipos de página

Todas llevan frontmatter YAML (para Obsidian/Dataview) y enlazan con `[[ruta/sin-extension]]`.

- **`apps/<binario>.md`** — una por app (`apps/kcalc.md`). Secciones: *Resumen* · *Cómo lanzarla* ·
  *Mapa de la interfaz* (estados importantes y cómo llegar) · *Controles* (qué tipo de control abre cosas
  en esta app) · *Cobertura acumulada* (tabla por corrida) · *Hallazgos* (enlaces a `bugs/`) ·
  *Pendiente* (qué falta probar y con qué parámetros). Termina **siempre** con el bloque de máquina:

  ````
  ```mcpos-explore
  {"launch": "kcalc", "deny": [], "seeds": [[{"role": "menu", "name": "Settings", "action": "Press"}]]}
  ```
  ````
  `explore__start(app="kcalc")` lee este bloque: usa `launch`, añade `deny` y arranca Go-Explore desde
  `seeds`. Cópialas de la sección *Seed paths* del digest (máx. 8, las más profundas y distintas).
  JSON válido en una sola línea o varias; nada de comentarios dentro.

- **`bugs/<id>.md`** — un hallazgo único (el `id` de `explore__findings`). Frontmatter: `app`, `kind`
  (crash/hang/exit/log-error), `status` (`nuevo` → `reproducido` → `reportado` → `corregido-upstream` |
  `no-reproducible` | `no-es-bug`), `signature`, `first_seen`, `runs`. Cuerpo: qué pasa, pasos mínimos,
  pila (solo marcos relevantes), resultado de `explore__replay`, enlace al reporte upstream.
- **`toolkits/<nombre>.md`** — rarezas de Qt, GTK, Firefox/Gecko, KWin, etc. que afectan a la
  automatización (p. ej. acciones AT-SPI que no hacen nada). Cada afirmación lleva su fuente.
- **`concepts/<tema>.md`** — ideas de método (Go-Explore, estados fantasma, oráculos…).
- **`overview.md`** — una página: qué apps se han explorado, cuántos bugs, qué aprendimos, qué sigue.

## Operaciones

### Ingesta (tras cada corrida de `explore__start`)
1. `explore__digest(run)` → crea `raw/runs/<run>.md`. Léelo; es la fuente.
2. Actualiza o crea `apps/<app>.md`: añade una fila a *Cobertura acumulada*, revisa *Mapa*, *Controles*
   y *Pendiente*, y renueva el bloque `mcpos-explore` (semillas nuevas, `deny` si algo molestó).
3. Por cada hallazgo: crea `bugs/<id>.md` o, si ya existe, suma la corrida y el conteo. Si es nuevo
   y grave (crash/hang), ejecuta `explore__replay(id, times=3)` y anota el resultado.
4. Si aprendiste algo del toolkit o del método, actualízalo en `toolkits/` o `concepts/`.
5. Actualiza `index.md` y `overview.md`; añade a `log.md`:
   `## [AAAA-MM-DD] ingest | <run> | <app> | <n> hallazgos`
6. Normalmente una ingesta toca entre 3 y 8 páginas. Si tocas más de 15, detente y revisa.

### Consulta
Lee `index.md` primero y luego solo las páginas necesarias; responde citando `[[páginas]]` y
`raw/…`. Si la respuesta tiene valor duradero (una comparación, un análisis), **guárdala** como página
nueva y regístrala: `## [AAAA-MM-DD] query | <pregunta corta>`.

### Lint (cuando el humano lo pida o cada ~5 ingestas)
1. Ejecuta `mcpos-wiki-lint` (gratis: enlaces rotos, páginas huérfanas, bloques `mcpos-explore`
   inválidos, bugs sin `status`, apps sin corridas recientes).
2. Revisa a mano lo que la herramienta no ve: contradicciones entre páginas, afirmaciones que una
   corrida más nueva dejó viejas, **páginas duplicadas** del mismo concepto con otro nombre (estados o
   nodos fantasma: fusiónalas y deja una redirección), conceptos sin página.
3. Propón las siguientes corridas (app, minutos, semillas) para cubrir huecos.
4. `## [AAAA-MM-DD] lint | <resumen>` en `log.md`.

## Reglas

- **Nada sin fuente.** Cada afirmación remite a un `raw/…` o a otra página. Si algo es una hipótesis,
  escríbelo como hipótesis.
- **Las contradicciones no se resuelven sobrescribiendo.** Si una corrida nueva contradice una vieja,
  deja las dos, marca `> [!warning] Contradicción` y explica qué cambió (versión, semilla, parámetros).
- **No dupliques**: antes de crear una página busca en `index.md` y con `grep -ri` un nombre parecido.
- Nombres de archivo en minúsculas con guiones; `apps/` y `toolkits/` usan el nombre del binario
  o del toolkit.
- No copies salidas largas: la pila completa vive en `explore__finding`; aquí basta con los marcos
  que explican el bug.
- La wiki es un repo git: `git -C ~/mcpos-wiki add -A && git -C ~/mcpos-wiki commit -m "<operación>"`
  al final de cada operación.
