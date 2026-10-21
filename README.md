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
| Azul continua, flecha en el destino | Link **manual** dibujado por ti. Se guarda en `state.json` |
| Ámbar discontinua, flecha en el destino | Link **detectado** a partir del tráfico. No se guarda; desaparece tras **30 s** sin actividad |
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
| `terminal_has_foreground_process` | `terminalId` | `bool` (siempre `false` en Windows) |
| `attach_terminal` | `terminalId`, `cols`, `rows` | `AttachResult`: `{ data: base64, seq, alive }` |
| `restart_terminal` | `terminalId`, `cols`, `rows` | nada |
| `write_terminal` | `terminalId`, `data` (**base64**) | nada |
| `resize_terminal` | `terminalId`, `cols`, `rows` | nada |
| `create_link` | `sourceId`, `targetId` | `Link` manual |
| `delete_link` | `linkId` | nada |

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

## Detección de links

En `src-tauri/src/links.rs`, dos señales crean o refrescan un link **detectado** de A a B (siempre dentro del mismo workspace):

1. **Mención.** La salida de A contiene el **nombre** de B (sin distinguir mayúsculas, como palabra completa, de al menos 3 caracteres) o su **id**. Antes se quitan las secuencias de escape, y un nombre partido entre dos lecturas también cuenta.
2. **Relay (reenvío).** El texto que se escribe en B lo produjo A en los últimos **4 s**. Cubre copiar la respuesta de un agente y pegarla o enviarla a otro. La entrada debe tener al menos 8 caracteres; se comparan sus últimos 64 tras normalizar espacios y mayúsculas.

Si ya existe un link en ese sentido (manual o detectado), se refresca su actividad en vez de duplicarlo. La detección funciona también en los workspaces que no están visibles.

Constantes ajustables (`links.rs`):

| Constante | Valor | Para qué |
|---|---|---|
| `DETECTED_LINK_TTL_MS` | 30 000 | Inactividad tras la que desaparece un link detectado |
| `INPUT_CORRELATION_WINDOW_MS` | 4 000 | Cuánto tiempo cuenta una salida para la señal de relay |
| `MIN_NAME_CHARS` | 3 | Los nombres más cortos se ignoran para las menciones |
| `MIN_RELAY_CHARS` | 8 | Una entrada más corta nunca dispara un relay |
| `LINK_ACTIVITY_THROTTLE_MS` | 1 000 | Un link ya visible envía como mucho una actualización por periodo, aunque el tráfico sea continuo |

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

El MVP se desarrolló y verificó en **Linux** (Arch, WebKitGTK 2.52). El código es multiplataforma y compila para Windows (`cargo check` / `cargo clippy --target x86_64-pc-windows-gnu` sin avisos), pero no se ha ejecutado en Windows ni en macOS.

### Windows

- **Sin confirmación al cerrar.** La comprobación de "hay un programa en primer plano" compara el grupo de procesos en primer plano del PTY (`tcgetpgrp`) con el del shell, y ConPTY no expone ese dato. En Windows, **×** cierra la terminal directamente, aunque haya un agente trabajando.
- **Cierre más simple.** En Unix se envía SIGHUP y, si el shell sigue vivo a los 2 s, SIGKILL a su grupo de procesos y al de primer plano. En Windows solo se termina el proceso del shell; lo que ocurra con sus procesos hijos depende de ConPTY y no está verificado.
- **Shell:** Windows PowerShell (`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`); si no existe, `%ComSpec%` (`cmd.exe`). PowerShell 7 (`pwsh`) no se usa por defecto.
- **Requisitos:** ConPTY (Windows 10 1809 o posterior) y el runtime de WebView2 (incluido en Windows 11).
- **Atajos de portapapeles:** WebView2 asocia Ctrl + Shift + V a su propio pegado. `TerminalView` cancela ese comportamiento para no pegar dos veces.

### Todas las plataformas

- **Pellizco de zoom en el trackpad.** En macOS se usan los eventos `gesture*` de WebKit y en Windows WebView2 lo entrega como `ctrl + rueda`; ambos casos están contemplados pero sin verificar. En **Linux**, WebKitGTK haría zoom de toda la página sin avisar a JS. Por eso el backend (`src-tauri/src/pinch.rs`) pone un `GtkGestureZoom` en fase de captura sobre la ventana: reclama el gesto antes que WebKit y lo reenvía como evento `canvas-pinch`. El lienzo hace zoom anclado en el centro del pellizco y la interfaz conserva su tamaño. Está verificado con pellizcos sintéticos de GDK; falta probarlo con un touchpad físico. **Ctrl + rueda** sigue funcionando en todas las plataformas.
- **Renderizado.** Se usa el renderer WebGL de xterm si está disponible. Si falta WebGL o se pierde el contexto, xterm vuelve a su renderer DOM; los navegadores limitan los contextos WebGL vivos a unos 16. xterm 6 ya no incluye el addon de canvas, por eso el respaldo es DOM. El texto WebGL se ve algo borroso con zoom por encima del 100 %; pon `USE_WEBGL = false` en `TerminalView.tsx` si prefieres nitidez a rendimiento.
- **TUIs a pantalla completa.** Al volver a un workspace, el buffer reproduce los bytes en bruto; un programa a pantalla completa se ve bien cuando vuelve a redibujarse (en su siguiente actualización o redimensionado).
- **Duplicar terminal.** `duplicate_terminal` abre la shell nueva en el directorio de trabajo del programa en primer plano del origen (o, si no hay ninguno, en el de su shell). Si hay un programa en primer plano, por ejemplo un agente, escribe su `argv` en la shell nueva, con cada argumento entre comillas simples, y pulsa Enter. Así el programa corre dentro de una shell y la shell sigue abierta cuando termina. Si algún argumento tiene caracteres de control, solo se abre la shell, porque el editor de línea los tomaría como teclas. Directorio y programa se leen de `/proc`, así que solo funciona en **Linux**. En macOS y Windows la copia es una shell en el directorio personal. No se copian los links ni el historial. Al reiniciar la app, la copia arranca en el directorio personal, como cualquier terminal.
- **Procesos.** Al cerrar la app se cierran todas las shells; no hay procesos en segundo plano que sobrevivan.

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
