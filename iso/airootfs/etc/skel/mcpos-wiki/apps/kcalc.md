---
type: app
app: kcalc
toolkit: qt
runs: 2
last_run: 2026-11-11
bugs: 0
sources: [raw/notes/2026-11-09-11-sesiones-iniciales.md]
---

# KCalc

## Resumen
Calculadora de KDE ([[toolkits/qt]]). Dos corridas de 30 min sin hallazgos, pero **la exploración aún no
es completa**: quedaron sin probar justo las funciones donde una calculadora suele fallar.

## Cómo lanzarla
`kcalc` (no es de instancia única; funcionó con HOME desechable). Viene en la ISO desde v0.5.1.

## Mapa de la interfaz
- Menús: File, Edit, Settings, Help, **Constants** (Mathematics, Electromagnetism, Atomic & Nuclear).
- Modos de base numérica: Dec / Hex / Oct / Bin, casilla Shift.
- Diálogos alcanzados: Configure Keyboard Shortcuts, About KDE, About KCalc, Show History, Report Bug.

## Controles
Combos, casillas, pestañas y entradas de menú: 50–64 % de novedad. Los botones del teclado casi nunca
cambian la estructura, pero son el objetivo real (ver [[concepts/go-explore]]). Acciones `SetFocus`: ruido.

## Cobertura acumulada
| fecha | versión | min | acciones | reinicios | estados | acciones probadas | hallazgos |
|---|---|---|---|---|---|---|---|
| 2026-11-11 | v0.5.1 | 30 | 3201 | 65 | 14 | — | 0 |
| 2026-11-11 | v0.5.2 | 30 | 3205 | 37 | 88 | 132/215 | 0 |

(raw/notes/2026-11-09-11-sesiones-iniciales.md)

## Hallazgos
Ninguno todavía.

## Pendiente
- Repetir 30 min con v0.5.3 (ignora SetFocus, bono por acción no probada) y mirar *actions tried X/Y*.
- Sin probar: x!, xʸ, x^(1/y), raíces, nCm, mod; AND/OR/XOR/Lsh/Rsh/Cmp/IntDiv (modo de bases);
  memoria; OK/Cancel/Defaults de los diálogos.
- Ideas de casos límite (hipótesis, no verificadas): factorial grande, xʸ con extremos, IntDiv entre 0,
  desplazamientos en Hex/Bin.
- Tras la próxima corrida: copiar aquí las *Seed paths* del digest.

```mcpos-explore
{"launch": "kcalc", "deny": ["Launch Bug Report Wizard"], "seeds": []}
```
