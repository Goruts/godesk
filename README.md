# Godesk

Un lienzo infinito de terminales reales. Pon cada agente de código (Claude Code, Codex, OpenCode, …) en su propia terminal, colócalas en el espacio, agrúpalas en workspaces y **ve qué agentes se están comunicando** gracias a las líneas de conexión entre sus ventanas.

Es una aplicación de escritorio: un backend en Rust (Tauri 2) ejecuta shells reales en pseudoterminales (PTY), y un frontend en React + xterm.js se muestra en el webview del sistema.

## Inicio rápido

Requisitos:

| Herramienta | Versión |
|---|---|
| Rust | estable, 1.85 o superior (edición 2024) |
| Node.js | 22.12 o superior (lo exige Vitest) |
| Dependencias de sistema de Tauri 2 | ver [tauri.app/start/prerequisites](https://tauri.app/start/prerequisites/). En Linux: `webkit2gtk-4.1`, `libsoup-3.0` y `gtk3` |

```bash
npm install      # instala la CLI de Tauri y, en postinstall, las dependencias de frontend/
npm run dev      # arranca Vite en http://localhost:5173 y la app de escritorio con recarga en caliente
```

Resultado esperado: se abre una ventana con un lienzo punteado. Pulsa **Ctrl/⌘ + Shift + T** para abrir una terminal con tu shell en `$HOME`.

Otros comandos (desde la raíz):

| Comando | Qué hace |
|---|---|
| `npm run build` | Binario de release e instaladores en `src-tauri/target/release/bundle/` |
| `npm test` | Tests de Rust y después tests del frontend |
| `npm run gen:types` | Regenera los tipos TypeScript a partir del modelo Rust (ver [Generación de tipos](#generación-de-tipos)) |

> El puerto 5173 es fijo (`strictPort`). Si está ocupado, `npm run dev` falla en lugar de usar otro puerto, porque Tauri carga exactamente `devUrl` (`src-tauri/tauri.conf.json`).

## Uso

| Acción | Cómo |
|---|---|
| Nueva terminal | Botón **+ New terminal** o **Ctrl/⌘ + Shift + T**. Aparece centrada en la zona visible, en cascada si ya hay una en ese sitio |
| Desplazar el lienzo | Rueda o trackpad sobre el fondo, arrastre con el botón central en cualquier sitio, o **Espacio + arrastrar** |
| Zoom | **Ctrl/⌘ + rueda**, centrado en el cursor (10 % – 400 %). Botones **−** y **+**; el botón del porcentaje vuelve a 100 % |
| Encuadrar todo | **Fit to content** |
| Mover / redimensionar | Arrastra la barra de título / cualquier borde o esquina (mínimo 320 × 200) |
| Renombrar | Doble clic en el título (terminal) o en el nombre (workspace). En workspaces también el lápiz o **F2** |
| Enfocar | Clic en una terminal: pasa al frente y recibe el teclado |
| Historial | Rueda dentro de la terminal (el lienzo no se mueve) |
| Copiar / pegar | **Ctrl/⌘ + Shift + C / V** dentro de la terminal |
| Cerrar | **×** en la barra de título. Si hay un programa distinto del shell en primer plano, se pide confirmación (solo Unix) |
| Reiniciar un shell terminado | Botón **Restart shell** sobre la terminal |
| Conectar dos terminales | Arrastra el asa **●** del borde derecho de una ventana hasta otra ventana. **Esc** cancela |
| Borrar un link | Clic en la línea y **Supr** (o **Retroceso**, o el botón **Delete link**). **Esc** quita la selección |
| Mostrar / ocultar links | **Hide links / Show links** |
| Workspaces | Barra lateral: crear, cambiar, renombrar y borrar (con confirmación). El último no se puede borrar |

Cambiar de workspace no mata procesos: las terminales ocultas siguen ejecutándose, su salida se sigue guardando y la detección de links sigue activa para ellas.

## Leyenda de conexiones

| Línea | Significado |
|---|---|
| Blue solid, arrowhead at the target | A link drawn by hand, or by Godesk the first time an agent delegates to another terminal. Saved in `state.json` |
| Más gruesa, animada y con halo | Actividad reciente (últimos 5 s), sea manual o detectado |
| Atenuada | Sin actividad reciente |
| Blanca | Seleccionada (Supr para borrarla) |
| Dos curvas separadas | Hay links en ambos sentidos entre el mismo par |

Al pasar el cursor por una línea se muestra `origen → destino` y su tipo. Los links son solo visuales: nunca mueven datos entre terminales ni ejecutan nada.

## Arquitectura

```
src-tauri/                 Rust (Tauri 2)
  src/model.rs             Workspace, TerminalWindow, Camera, Link, eventos… — única definición del modelo
  src/state.rs             Modelo en memoria y todas las reglas de validación (puro, con tests)
  src/pty.rs               Sesiones de shell con portable-pty (PTY en Unix, ConPTY en Windows)
  src/scrollback.rs        Buffer de reproducción por terminal (5000 líneas / 1 MB)
  src/links.rs             Detección heurística de links (pura, con tests)
  src/text.rs              Decodificación UTF-8 en streaming, limpieza de secuencias ANSI, búsqueda de palabras
  src/persist.rs           Carga y guardado de state.json (con debounce y escritura atómica)
  src/core.rs              Unión: eventos al frontend y detección sobre salida/entrada
  src/commands.rs          Handlers #[tauri::command]
  src/pinch.rs             Linux: reenvía el pellizco del touchpad al lienzo (evento canvas-pinch)
frontend/                  React 19 + TypeScript estricto + Vite + Zustand + xterm.js 6
  src/bindings/            Generado desde el modelo Rust con ts-rs. No editar a mano
  src/lib/api.ts           Envoltorios tipados de los comandos y eventos
  src/canvas/              Cámara, geometría, registro de layout y viewport (sin React)
  src/components/          Canvas, TerminalWindow, TerminalView (xterm), LinkLayer (SVG), Sidebar, Toolbar
  src/store/               Stores de Zustand (modelo de la app, diálogo de confirmación)
```

Cada shell arranca en el directorio personal con `TERM=xterm-256color`, `COLORTERM=truecolor` y tres variables propias: `GODESK_TERMINAL_ID`, `GODESK_TERMINAL_NAME` y `GODESK_WORKSPACE_ID`. Así un agente puede referirse a los demás por su nombre.

## Diseño de la comunicación

### Comandos (frontend → backend)

El frontend llama con `invoke` a estos `#[tauri::command]`. Todos validan en `state.rs` antes de tocar procesos, y programan un guardado si cambian el modelo.

| Comando | Parámetros | Devuelve |
|---|---|---|
| `get_state` | — | `AppSnapshot` completo (incluye links detectados) |
| `create_workspace` | `name?` | `Workspace` |
| `rename_workspace` | `workspaceId`, `name` | nada |
| `delete_workspace` | `workspaceId` | id del nuevo workspace activo. Cierra sus shells |
| `set_active_workspace` | `workspaceId` | nada |
| `update_camera` | `workspaceId`, `camera` | nada |
| `create_terminal` | `workspaceId`, `rect` | `TerminalWindow`, con su shell ya arrancada |
| `duplicate_terminal` | `terminalId`, `rect` | `TerminalWindow` copia, en el workspace del origen y con nombre `<origen> N` (ver abajo) |
| `close_terminal` | `terminalId` | nada. Borra también sus links |
| `rename_terminal` | `terminalId`, `name` | nada |
| `update_terminal_rect` | `terminalId`, `rect` | `Rect` saneado (mínimo 320 × 200) |
| `focus_terminal` | `terminalId` | nuevo `z` |
| `terminal_has_foreground_process` | `terminalId` | `bool`. En Windows, si el shell tiene un programa hijo vivo |
| `terminal_folder` | `terminalId` | carpeta del programa en primer plano (o del shell), o `null` |
| `attach_terminal` | `terminalId`, `cols`, `rows` | `AttachResult`: `{ data: base64, seq, alive }` |
| `restart_terminal` | `terminalId`, `cols`, `rows` | nada |
| `write_terminal` | `terminalId`, `data` (**base64**) | nada |
| `resize_terminal` | `terminalId`, `cols`, `rows` | nada |
| `create_link` | `sourceId`, `targetId` | `Link` manual |
| `delete_link` | `linkId` | nada |
| `pick_path` | `folder` | ruta elegida en el diálogo de carpeta (o de archivo), o `null` si se cancela |
| `list_windows` | — | `NativeWindow[]`: ventanas abiertas que se pueden traer al lienzo (solo Windows) |
| `open_app` | `launch` (`AppLaunch`) | `EmbeddedApp`, cuando su ventana ya apareció (ver "Programas en el lienzo") |
| `place_apps` | `placements` (`AppPlacement[]`) | nada |
| `focus_app` | `appId` | nada |
| `close_app` | `appId` | nada. El programa decide; el evento `app-changed` avisa cuando cerró |
| `release_app` | `appId` | nada. Devuelve la ventana al escritorio |

- Los errores llegan como un string con el mensaje (por ejemplo `the last workspace cannot be deleted`).
- Los comandos que no devuelven nada resuelven a **`null`**, no a `undefined`. Tenlo en cuenta al distinguir éxito de error en el frontend y en los mocks de tests.
- Los nombres se recortan, no pueden quedar vacíos y se limitan a 64 caracteres.

### Eventos (backend → frontend)

| Evento | Payload |
|---|---|
| `pty-output-<terminalId>` | `{ data: base64, seq }` |
| `pty-exit-<terminalId>` | `{ exitCode }` |
| `links-changed` | `{ workspaceId, links }`: la lista completa de links de ese workspace |
| `canvas-pinch` | `{ phase: begin \| update \| end, scale, x, y }`: solo en Linux. `scale` es acumulada desde `begin`; `x`/`y` son coordenadas client del centro del gesto |

### Por qué este diseño

- **Base64 en vez de strings.** Una lectura del PTY puede terminar a mitad de un carácter UTF-8 o de una secuencia de escape. Convertir cada trozo en un string de JS rompería esos caracteres. Los bytes viajan intactos y xterm los reensambla. La entrada viaja igual por el mismo motivo.
- **Un evento por terminal.** Cada vista se suscribe solo a su terminal, así un agente con mucha salida no despierta al resto de vistas.
- **`seq` hace la reconexión exacta.** Cada trozo lleva el total de bytes que ha producido la sesión hasta su final. Una vista primero se suscribe a su evento y después llama a `attach_terminal`, que devuelve el buffer retenido y el `seq` en el que termina. Los eventos con `seq ≤` ese valor ya están en el buffer y se descartan. No se pierde ni se duplica nada.
- **La entrada nunca bloquea la UI.** Las escrituras van a un hilo escritor por sesión a través de un canal. Un proceso que deja de leer no congela el hilo principal.
- **Los comandos y los eventos no están ordenados entre sí.** Un `pty-exit` puede llegar antes que la respuesta de `attach_terminal`. La vista guarda su propio estado de "terminado" y no deja que la respuesta lo deshaga.

Cada terminal conserva su salida reciente (hasta **5000 líneas o 1 MB**, recortando por líneas completas). Ese buffer se reproduce cuando su workspace vuelve a mostrarse.

### Renderizado del lienzo

- El mundo es un contenedor con `transform: translate(x, y) scale(zoom)`. Posiciones y tamaños se guardan en **coordenadas de lienzo**, así que mover y redimensionar dividen el desplazamiento del puntero por el zoom.
- Pan, zoom, arrastre y redimensionado escriben directamente en el DOM (`viewport.ts`, `layout.ts`). React solo vuelve a renderizar al confirmar un cambio.
- Los links son paths SVG dentro del contenedor transformado, por debajo de las ventanas. Su geometría se recalcula de forma imperativa (agrupada con `requestAnimationFrame`) cuando una ventana se mueve: las líneas siguen el arrastre **sin un render de React por frame**. El grosor se escala con `1 / zoom` para que se vea igual a cualquier zoom.
- Mientras arrastras, redimensionas, desplazas o dibujas un link, una capa a pantalla completa y la captura del puntero impiden que xterm se quede con el ratón.
- xterm calcula la posición del ratón con desplazamientos escalados pero celdas sin escalar. `TerminalView` reenvía los eventos de ratón con coordenadas corregidas por el zoom, para que la selección caiga en la celda correcta a cualquier zoom.

## Generación de tipos

El modelo se define una sola vez, en `src-tauri/src/model.rs`, con `serde` + [`ts-rs`](https://github.com/Aleph-Alpha/ts-rs). Cada tipo marcado con `#[ts(export)]` genera un archivo en `frontend/src/bindings/`:

```bash
npm run gen:types    # = cd src-tauri && cargo test export_bindings
```

- `src-tauri/.cargo/config.toml` apunta `TS_RS_EXPORT_DIR` a `frontend/src/bindings`. Cargo lee ese archivo desde el directorio actual, así que ejecuta `cargo` **desde dentro de `src-tauri/`**. Con `--manifest-path` desde la raíz, los bindings acaban en `src-tauri/bindings/`.
- Cualquier `cargo test` también los regenera.
- Solo se generan los **tipos**. Los envoltorios de comandos y eventos se escriben a mano en `frontend/src/lib/api.ts`: si añades o cambias un comando, actualiza ese archivo.
- Los `u64` (`seq`, `lastActivityAt`) se exportan como `number`. Son milisegundos o bytes, muy por debajo de 2^53.

## Persistencia

`state.json` guarda los workspaces, las terminales (nombre, posición, tamaño y orden z), la cámara de cada workspace, el workspace activo y los links **manuales**. Vive en el directorio de datos de la app (`app_data_dir` de Tauri):

| Sistema | Ruta |
|---|---|
| Linux | `$XDG_DATA_HOME/com.godesk.app/state.json` (por defecto `~/.local/share/com.godesk.app/`) |
| macOS | `~/Library/Application Support/com.godesk.app/state.json` |
| Windows | `%APPDATA%\com.godesk.app\state.json` |

- Los guardados tienen debounce (400 ms) y son atómicos (archivo temporal + rename). Al salir se fuerza un guardado.
- Al arrancar, cada terminal guardada recibe una **shell nueva** en su posición. Los procesos y su salida no sobreviven a un reinicio.
- Un archivo que no se puede leer o parsear se mueve a `state.json.corrupt-<timestamp>` y la app arranca con un workspace vacío. El aviso solo sale por stderr. Los links colgantes y los valores fuera de rango se reparan al cargar.
- Para probar sin tocar tu estado real en Linux: `XDG_DATA_HOME=/tmp/godesk-prueba npm run dev`.

## Links and delegation

An arrow between two terminals is a channel the agents in them can talk over. There are two ways to get one:

1. **Drawn by hand.** Drag the handle **●** on the right edge of a window onto another window.
2. **Drawn by Godesk when an agent delegates.** Any terminal can reach every other terminal of **its own workspace** by name, with no arrow drawn first. The first time it hands work to another one with `godesk send`, Godesk draws the arrow sender → receiver; the first `godesk read` draws read terminal → reader. The canvas then shows who instructs whom, and the receiver can answer back by name.

Both kinds are the same kind of arrow: saved in `state.json`, lit up while traffic flows over them, and deletable. Terminals of different workspaces never reach each other. Only an explicit `godesk` call draws an arrow; nothing is guessed from a terminal's output (`src-tauri/src/bridge.rs`, `connect`).

**The `godesk` command.** Every shell has a `godesk` command on its PATH so that whatever runs in a terminal (Claude Code, Codex or any other program) can reach another one:

```
godesk list                        # the other terminals of the workspace, what runs in each, [connected] when an arrow exists
godesk send <terminal> <message>   # types the message into that terminal and presses Enter
godesk read <terminal> [lines]     # prints the end of its output
```

A terminal is named without regard to case, or by the only terminal whose name contains the text. A message of several lines is sent as one, and Enter goes 150 ms later so that TUIs such as Claude Code take it as a submission rather than a paste. Behind it is an HTTP endpoint on `127.0.0.1` (`src-tauri/src/bridge.rs`) with a random token that only Godesk's shells know (`GODESK_PORT`, `GODESK_TOKEN`). The scripts (`godesk` for sh/Git Bash with curl, `godesk.ps1` for PowerShell and `godesk.cmd` for cmd.exe) are written to `<app data>/bin` on every start.

**Orchestrating.** An agent only uses `godesk` if it knows it exists. With ⋯ → *Agents know the terminals by name*, Godesk adds a note between `<!-- godesk:begin -->`/`<!-- godesk:end -->` markers to the global instructions of Claude Code (`~/.claude/CLAUDE.md`) and Codex (`~/.codex/AGENTS.md`). It tells them the title-bar names are terminals of the canvas, how to use `godesk`, and that when asked to lead or coordinate other terminals they should hand each one its part with `godesk send`, follow up with `godesk read` and ask them to report back. Turning it off removes only that block (`src-tauri/src/agents.rs`).

Adjustable constant (`links.rs`):

| Constant | Value | Purpose |
|---|---|---|
| `LINK_ACTIVITY_THROTTLE_MS` | 1 000 | A visible arrow sends at most one update per period, even under continuous traffic |

### Límites y falsos positivos conocidos

Es una heurística: muestra coordinación probable, no una prueba.

- **Eco del shell.** Escribir `# luego pregunto a planner` en A, aunque no lo ejecutes, hace que el shell lo muestre en pantalla, y aparece A → planner.
- **Mismo comando en varias terminales.** Escribir en B un comando que A acaba de mostrar (por ejemplo, lanzar el mismo comando en todas las terminales, o que ambas hagan `git status`) cuenta como relay. Con seis terminales y el mismo comando aparecen links entre todas.
- **Nombres comunes.** Una terminal llamada `test`, `build` o `server` coincide con salida corriente. Usa nombres distintivos.
- **Menciones incidentales.** Prompts, rutas, la salida de `ls` o `env`, o logs que contengan el nombre o el id de otra terminal también cuentan.
- **Interacciones invisibles.** Agentes que se coordinan por archivos, sockets o APIs sin escribir el nombre del otro no se detectan.
- **Entorno desactualizado.** Al renombrar una terminal la detección usa el nombre nuevo al momento, pero el shell en marcha conserva el `GODESK_TERMINAL_NAME` anterior hasta que se reinicia.
- **Nombres duplicados.** Dos terminales con el mismo nombre reciben ambas el link de una mención.
- Los links son dirigidos y no cruzan workspaces. El backend rechaza links manuales hacia la propia terminal, hacia otro workspace o hacia una terminal inexistente.

## Plataformas y limitaciones

El MVP se desarrolló y verificó en **Linux** (Arch, WebKitGTK 2.52). En **Windows 10** se verificaron duplicar terminal y los programas en el lienzo (tests en ConPTY real y la app compilada). macOS no se ha ejecutado.

### Windows

- **Programa en primer plano.** ConPTY no expone un grupo de procesos en primer plano. `src-tauri/src/winproc.rs` toma el hijo más reciente del shell en la tabla de procesos (mirando a través de envoltorios `cmd.exe /c`, como los `.cmd` que instala npm para `claude` o `codex`) y lee su carpeta y su línea de comandos del PEB del proceso. Con eso funcionan duplicar y la confirmación al cerrar.
- **Cierre.** Al cerrar una terminal se terminan primero los programas que lanzó su shell y después el shell, para que un agente no quede corriendo sin ventana.
- **ConPTY propio.** La ConPTY de Windows 10 pierde líneas cuando la salida avanza más rápido de lo que dibuja: la conversación anterior de un agente no llegaba al historial y había que redimensionar la terminal para verla. Godesk instala junto a `godesk.exe` el `conpty.dll` y el `OpenConsole.exe` de Windows Terminal (`src-tauri/conpty/`, MIT, firmados por Microsoft), que `portable-pty` carga antes que el del sistema.
- **Shell:** Windows PowerShell (`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`); si no existe, `%ComSpec%` (`cmd.exe`). PowerShell 7 (`pwsh`) no se usa por defecto. PowerShell arranca con `-NoExit -EncodedCommand` y un envoltorio de `prompt` que copia la ubicación actual a `[Environment]::CurrentDirectory`; sin eso, `cd` no cambia el directorio del proceso y una copia no sabría dónde está el shell. El `prompt` del perfil (oh-my-posh, etc.) se conserva.
- **Requisitos:** ConPTY (Windows 10 1809 o posterior) y el runtime de WebView2 (incluido en Windows 11).
- **Ctrl + Alt + F** (atajo global) muestra Godesk maximizado desde cualquier lado, o lo minimiza si ya está al frente. Si otro programa tiene el atajo cuando Godesk arranca, lo reintenta cada 5 s hasta que quede libre.
- **Atajos de portapapeles:** WebView2 asocia Ctrl + Shift + V a su propio pegado. `TerminalView` cancela ese comportamiento para no pegar dos veces.

### Todas las plataformas

- **Pellizco de zoom en el trackpad.** En macOS se usan los eventos `gesture*` de WebKit y en Windows WebView2 lo entrega como `ctrl + rueda`; ambos casos están contemplados pero sin verificar. En **Linux**, WebKitGTK haría zoom de toda la página sin avisar a JS. Por eso el backend (`src-tauri/src/pinch.rs`) pone un `GtkGestureZoom` en fase de captura sobre la ventana: reclama el gesto antes que WebKit y lo reenvía como evento `canvas-pinch`. El lienzo hace zoom anclado en el centro del pellizco y la interfaz conserva su tamaño. Está verificado con pellizcos sintéticos de GDK; falta probarlo con un touchpad físico. **Ctrl + rueda** sigue funcionando en todas las plataformas.
- **Renderizado.** Se usa el renderer WebGL de xterm si está disponible. Si falta WebGL o se pierde el contexto, xterm vuelve a su renderer DOM; los navegadores limitan los contextos WebGL vivos a unos 16. xterm 6 ya no incluye el addon de canvas, por eso el respaldo es DOM. El texto WebGL se ve algo borroso con zoom por encima del 100 %; pon `USE_WEBGL = false` en `TerminalView.tsx` si prefieres nitidez a rendimiento.
- **TUIs a pantalla completa.** Al volver a un workspace, el buffer reproduce los bytes en bruto; un programa a pantalla completa se ve bien cuando vuelve a redibujarse (en su siguiente actualización o redimensionado).
- **Duplicar terminal.** `duplicate_terminal` abre la shell nueva en el directorio de trabajo del programa en primer plano del origen (o, si no hay ninguno, en el de su shell). Si hay un programa en primer plano, por ejemplo un agente, escribe su `argv` en la shell nueva, con cada argumento entre comillas simples, y pulsa Enter. Así el programa corre dentro de una shell y la shell sigue abierta cuando termina. Si algún argumento tiene caracteres de control, solo se abre la shell, porque el editor de línea los tomaría como teclas. Directorio y programa se leen de `/proc` en **Linux** y de la tabla de procesos en **Windows**, donde la línea se escribe para PowerShell (`& 'programa' 'arg'…`) o `cmd.exe`; un argumento que PowerShell 5.1 pasaría mal (vacío o con `"`) hace que solo se abra la shell. En macOS la copia es una shell en el directorio personal. No se copian los links ni el historial. Al reiniciar la app, la copia arranca en el directorio personal, como cualquier terminal.
- **Reabrir donde estabas.** Cada 15 s y al cerrar, Godesk guarda por terminal su carpeta, su salida y lo que corre (`<datos de la app>/sessions/<id>.json` y `.log`, `src-tauri/src/session.rs`). Al arrancar, cada terminal vuelve en esa carpeta, con su salida anterior y una línea que marca el reinicio, y su programa (`src-tauri/src/resume.rs`): **Claude Code** con `--resume <id>` de su propia conversación (el `.jsonl` de `~/.claude/projects/<carpeta>` que cambió desde que arrancó; dos agentes en la misma carpeta se reparten conversaciones distintas), **Codex** con `codex resume <id>` (el `rollout-*.jsonl` de `~/.codex/sessions` de esa carpeta), **OpenCode** con `--continue`, y otros CLIs de agentes (gemini, aider, qwen…) tal cual. Cualquier otro programa queda escrito en la línea de comandos **sin Enter**: un build o un deploy no se repite solo.
- **Terminales de agente.** Escribir `claude`, `codex` u `opencode` (solo el nombre) en una terminal abre una ventana propia para ese agente, al lado y en la misma carpeta. El agente corre en su modo JSON y Godesk dibuja la conversación en lugar de su pantalla, igual para los tres: su razonamiento en vivo (abierto mientras piensa, plegado al terminar), el texto que escribe en streaming con markdown y bloques de código copiables, cada herramienta como tarjeta (comandos con su salida como una pequeña terminal, ediciones como diff `+`/`−`, archivos nuevos como código, el plan con lo hecho), y al final de cada turno su duración, tokens y coste. **Esc** o **Stop** detiene el turno sin perder la conversación; **↑** recupera el último mensaje. Claude Code mantiene un proceso (`--input-format stream-json`) y se reinicia con `--resume`; Codex (`exec --json`, sandbox `workspace-write`) y OpenCode (`run --format json`) arrancan un proceso por turno que continúa su sesión, con el mensaje por la entrada estándar (`src-tauri/src/agent_terminal.rs`, `agent_session.rs`, `frontend/src/agent/protocol.ts`).
- **Subagentes y tokens.** Cada 2 s Godesk lee los archivos de sesión de los agentes de las terminales (`src-tauri/src/agent_watch.rs`, solo lo que se agregó desde la última vez). Los subagentes de Claude Code (`<sesión>/subagents/agent-<id>.jsonl` y `.meta.json`) aparecen como tarjetas de solo lectura junto a su agente, unidas a él con un link que se ilumina mientras trabajan: no son terminales porque corren dentro del proceso de Claude. Muestran su tipo, su tarea, sus tokens y lo que dicen y las herramientas que llaman. Un subagente terminado se ve 10 minutos; **×** lo oculta. El dock muestra los tokens de todos los agentes juntos (Claude Code: entrada + caché + salida de cada mensaje, contado una vez; Codex: el total que registra), con el desglose por terminal. Codex no lanza subagentes.
- **Cerrar.** La **×**, Alt+F4 y *Quit* preguntan antes si alguna terminal corre un programa, porque cerrar lo detiene (`frontend/src/components/closeGuard.ts`). Al cerrar se guardan las terminales y se devuelven al escritorio las ventanas de otros programas; al cerrar la app se cierran todas las shells.

### Programas en el lienzo (Windows)

El botón de ventana del dock abre en el lienzo **VS Code** (en una carpeta), el **Explorador de archivos**, un **documento** con su programa, o trae una **ventana ya abierta**. El botón `</>` de cada terminal abre VS Code en la carpeta de esa terminal, a su lado.

- **Desde una terminal.** Lo que se lanza en una terminal de Godesk también aparece en el lienzo, junto a ella: `code .`, `explorer .`, `notepad`, o cualquier ventana de un proceso que cuelgue de su shell. El backend revisa los procesos de cada shell cada 200 ms. VS Code y el Explorador entregan la ventana a un proceso que ya corría, así que se reconocen por haberse lanzado: se toma su ventana nueva, o, si VS Code ya tenía esa carpeta abierta, la ventana que la muestra. Se desactiva en ⋯ → *Open programs from terminals here*.
- **Ventanas que se reemplazan.** Durante 10 s después de lanzarse, si el programa cierra su ventana y abre otra (VS Code reiniciándose para aplicar una actualización, una ventana restaurada que se cierra), la nueva entra en la misma tarjeta. Cerrarlo con la **×** de la tarjeta no espera reemplazo.

- **Cómo se ve.** La ventana del programa no se incrusta como hija: la de Godesk es transparente (DWM blur-behind) y una hija dibujada con GDI se vería traslúcida y por debajo del webview. Queda como ventana de nivel superior, **sin marco y con Godesk como dueño**: se mantiene encima de Godesk, se oculta al minimizarlo y sale de la barra de tareas. `src-tauri/src/embed.rs` la mueve sobre su tarjeta y la recorta (`SetWindowRgn`) a lo visible del lienzo, dejando huecos donde están el dock, los menús, los diálogos y las ventanas que van delante. `frontend/src/canvas/appPlacement.ts` manda esas posiciones una vez por cuadro.
- **Zoom.** El contenido de otro programa no se puede escalar: se dibujaría a tamaño normal, recortado, mientras todo lo demás se achica. Por debajo del 75 % de zoom, o si la tarjeta queda más chica que 480 × 320 px en pantalla, la ventana se oculta y la tarjeta muestra una foto de cómo se veía en ese momento (leída de la pantalla, solo si Godesk o la ventana están al frente; si no, el programa y su título); un clic en ella vuelve al 100 % centrado en la tarjeta. La foto no es en vivo: se renueva cada vez que la ventana se vuelve a ocultar. (Se probó mostrar una vista previa escalada: `PrintWindow` devuelve VS Code en blanco, y una ventana fuera de pantalla deja de dibujarse.)
- **Al cerrar Godesk** cada ventana vuelve al escritorio tal como estaba (estilo, dueño y posición). Las tarjetas no se guardan en `state.json`. **↗** en la barra de la tarjeta devuelve una sola; **×** le pide al programa que cierre, y este puede preguntar si guardar.
- **Límites.** Un programa que corre como administrador no se puede traer si Godesk no lo es. Las apps UWP/WinUI (el Bloc de notas de Windows 11, Configuración) se dibujan en otro proceso y pueden no comportarse bien. Si un programa ya tenía esa carpeta abierta (VS Code reutiliza la ventana), no aparece una ventana nueva: tráela desde "Windows already open".

## Tests y verificación

```bash
npm test                                # las dos suites siguientes
(cd src-tauri && cargo test)            # reglas del modelo, persistencia, detección, buffer y sesiones reales con /bin/sh
npm --prefix frontend test              # cámara, geometría, links, ventanas, sidebar y vista de terminal (Vitest)
npm --prefix frontend run typecheck
(cd src-tauri && cargo clippy --all-targets)
```

- `core::tests` ejecuta todo el backend sin ventana: dos shells reales, el sink de salida del PTY, la detección y el modelo, con un grabador en lugar de la app de Tauri (`core::Events`). Comprueba que una mención y una línea pegada crean links detectados, y que los eventos de salida llevan base64 con un `seq` creciente.
- `pty::tests::runs_the_default_shell_in_home` está marcado `#[ignore]` porque arranca tu shell de login real. Ejecútalo con `cargo test -- --ignored`.
- Los tests del frontend sustituyen `lib/api.ts` por mocks. Un comando sin retorno debe simularse resolviendo `null`, como hace Tauri: con `undefined` los tests pasan aunque la app real falle.

### Depurar el webview en Linux

WebKitGTK puede exponer su inspector remoto por HTTP. Con la app en modo debug:

```bash
WEBKIT_INSPECTOR_HTTP_SERVER=127.0.0.1:9223 ./src-tauri/target/debug/godesk   # con Vite ya corriendo en :5173
```

Abre `http://127.0.0.1:9223` para ver el objetivo inspeccionable. El mismo WebSocket (`/socket/1/1/WebPage`) permite ejecutar JavaScript dentro de la app real para pruebas de extremo a extremo.
