---
type: concept
updated: 2026-11-11
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# Go-Explore y la búsqueda sin tokens

`mcp-explore` es un explorador barato (Rust + AT-SPI) y el modelo solo entra en las excepciones.
Idea tomada de AlphaZero (aprender sin maestro, con recompensa objetiva) y del primer ANIMUS (curiosidad
pura en Pokémon Azul).

## Evolución medida
| versión | idea | KCalc real | mock |
|---|---|---|---|
| v0.5.1 | UCB con "acción sin probar = infinito" | 14 estados en 3201 acciones | — |
| v0.5.2 | Go-Explore (volver a estados poco visitados) + prioridad por tipo de control | 88 estados | crash ~3× antes (5 semillas) |
| v0.5.3 | ignorar SetFocus + bono por acción nunca probada | pendiente | crash 3/3 semillas (antes 1/3), 93–96 % acciones |

(raw/notes/2026-11-09-11-sesiones-iniciales.md)

## Lecciones
- Optimizar solo por **estados nuevos** ignora los botones que no cambian la pantalla, donde viven los
  bugs de una calculadora. Hace falta también **cobertura de acciones**.
- Las semillas de la wiki (bloque `mcpos-explore` de cada [[apps/kcalc|app]]) hacen que una corrida empiece
  donde terminó la anterior: el "libro de aperturas".
- Ver [[concepts/estados-fantasma]] y [[concepts/oraculo]].
