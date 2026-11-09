# mcpOS

Distro Linux de escritorio **MCP-native** para pruebas: un agente (Claude Code u otro cliente MCP) controla el sistema completo —shell como root, journal, coredumps y el escritorio Sway— a través de un único gateway MCP.

> ⚠️ Imagen de pruebas **solo para VM**: el usuario `mcp` tiene `sudo` sin contraseña, `ptrace_scope=0` y `perf_event_paranoid=-1`.

## Arquitectura

```
Claude Code ──stdio──► mcpd (gateway, audit log)
                         ├── shell    exec / spawn / read_log / system_info
                         ├── journal  query / list_boots / failed_units / coredumps / coredump_info / coredump_backtrace
                         └── desktop  get_tree / launch / sway_command / screenshot / key / type_text / pointer / clipboard / notify
```

- Las tools se exponen como `<servidor>__<tool>` (p. ej. `shell__exec`, `desktop__screenshot`).
- `mcpd__status` y `mcpd__restart` gestionan los servidores. Si un servidor muere, se relanza solo en la siguiente llamada.
- Cada llamada queda en `~/.local/state/mcpd/audit.jsonl` (alias `audit` en la VM para seguirla en vivo).
- Para añadir servidores (tuyos o de terceros), basta con una línea en `/etc/mcpd/servers.conf` o en `~/.config/mcpd/servers.conf`.

Todo está en Rust **sin dependencias externas**: `mcp-core` trae su propio JSON y su loop JSON-RPC. Compila offline y los binarios pesan ~450 KB. Si más adelante quieres pasarte a `serde_json`/`rmcp`, solo hay que cambiar `mcp-core`.

## Estructura

```
crates/
  mcp-core/     JSON, servidor MCP stdio, ejecución con timeout (mata el process group), base64, schemas
  mcpd/         gateway: spawn, handshake, agregación de tools, routing, auto-restart, audit
  mcp-shell/    shell sin restricciones (as_root, user, cwd, stdin, timeout)
  mcp-journal/  journalctl / systemctl --failed / coredumpctl (+ backtrace con gdb)
  mcp-desktop/  swaymsg IPC, grim, wtype, wl-clipboard, notify-send, puntero vía `seat cursor`
iso/
  packages.extra   paquetes que se añaden a releng (sway, firefox, gdb, bpftrace, perf, nodejs…)
  airootfs/        overlay: usuario mcp, autologin → sway, sudoers, sysctl de debug, coredumps, .mcp.json
  build.sh         releng + overlay + binarios → mkarchiso
  run-vm.sh        QEMU/KVM con virtio-gpu
```

## Uso

### Opción A: ISO con GitHub Actions (sin tener Arch)

1. Sube el repo a GitHub (`git remote add origin … && git push -u origin main`).
2. Actions → **build-iso** → *Run workflow*.
3. Al terminar (~15 min), descarga el artifact `mcpos-iso` (un .zip con la ISO).

### VMware Workstation (Windows)

Nueva VM → *I will install the OS later* → Linux / *Other Linux 6.x kernel 64-bit*:
- 4 vCPU, 6 GB RAM, disco 20 GB, red NAT
- *VM Options → Advanced → Firmware: UEFI* (Secure Boot **desactivado**)
- *Display → Accelerate 3D graphics* activado (si la pantalla queda negra, desactívalo: sway pasa a render por software)
- CD/DVD → la ISO

### Opción B: build local en un host con Arch

En un host con Arch:

```bash
cargo build --release          # opcional: build.sh también compila
sudo ./iso/build.sh            # ISO en out/mcpos-YYYY.MM.DD-x86_64.iso
./iso/run-vm.sh                # o NOGL=1 ./iso/run-vm.sh si no hay 3D en el host
```

Dentro de la VM arranca Sway con el usuario `mcp`. Abre una terminal (`Super+Enter`) y ejecuta:

```bash
mcpos-agent        # instala Claude Code la primera vez y lo abre en ~ (lee ~/.mcp.json → mcpd)
```

Aprueba el servidor `mcpd` cuando te lo pida, y prueba con algo como:

> "Abre firefox, toma un screenshot, y dime si hay errores en el journal de los últimos 5 minutos."

### Instalar en disco (persistente)

Desde el live (VM en **UEFI**, disco de 20 GB o más):

```bash
sudo mcpos-install              # lista discos
sudo mcpos-install /dev/sda     # GPT + ESP 1G + btrfs (@, @home, @log, @snapshots) + systemd-boot
```

Instala los mismos paquetes del live, copia mcpd/servidores/configuración y tu `/home/mcp` (incluido el login de Claude Code). Variables: `TZ_NAME`, `HOSTNAME_NEW`, `KEEP_HOME=0`.

### Probar sin ISO (en cualquier Linux con Sway)

```bash
cargo build --release
mkdir -p ~/.config/mcpd
sed "s#/usr/local/bin#$PWD/target/release#" iso/airootfs/etc/mcpd/servers.conf > ~/.config/mcpd/servers.conf
claude mcp add mcpd -- $PWD/target/release/mcpd
```

## Probado

- Sesión MCP completa contra `mcpd`: `initialize`, `tools/list` (24 tools), llamadas a shell, timeout con kill del process group, stdin UTF-8, servidor caído reportado en `status`, nombre de tool inválido y JSON roto → error -32700.
- ISO construida en GitHub Actions y arrancada en VMware Workstation (UEFI, 3D): autologin → Sway → `mcpos-agent` → Claude Code con `mcpd` (24 tools).
- `desktop__get_tree`, `launch`, `screenshot` y `journal__query` verificados end-to-end desde Claude Code.
- `mcpos-install` probado en VMware: arranque desde disco con btrfs + systemd-boot y sesión de Claude persistente.

## Siguientes pasos

1. `mcp-a11y`: árbol AT-SPI (botones, labels, estados) con `python-atspi`, que ya va en la ISO, para que el agente "lea" la UI sin screenshots.
2. `mcp-debug`: `strace -p`, `bpftrace` one-liners y `perf top` con salida resumida.
3. `mcp-dbus`: bridge genérico (`busctl call/introspect`).
4. Notificaciones MCP: stream de journal y coredumps nuevos hacia el cliente.
5. Transporte por unix socket / streamable HTTP en `mcpd`, para correrlo como servicio systemd y conectar varios clientes.
