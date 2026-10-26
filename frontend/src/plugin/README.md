# Godesk as a Goia surface

This folder builds godesk's infinite canvas as a **Goia plugin** ("surface") — an
installable board where every Goia agent session is a window you can place, with
lines showing which agents are talking.

It is decoupled from godesk's own backend: it runs inside a sandboxed frame in
Goia and reaches Goia's engine only through the host bridge (`sdk.ts`). The
standalone godesk app is untouched — this is a second build target that reuses
godesk's pure canvas modules (`../canvas/*`, `../components/LinkLayer`).

## Build

```bash
cd frontend
npm run build:plugin      # → frontend/dist-plugin/
```

The output `dist-plugin/` is the drop-in folder: `index.html`, `manifest.json`
and `assets/`.

## Install in Goia

1. In Goia: **Extensions → Install plugin**, and point at `frontend/dist-plugin/`.
   (Or copy the folder into Goia's data dir as `…/goia/extensions/godesk/`.)
2. Open **Agents** — a **Pizarra** tab appears, showing the board.

## The contract (what couples the two, and nothing else)

The only thing shared with Goia is the message protocol in `sdk.ts`, mirrored by
Goia's host in `src/lib/pluginHost.ts`:

- `frame → host`: `{ kind:"call", reqId, cmd, args }`
- `host → frame`: `{ kind:"result", reqId, ok, value|error }`
- `host → frame`: `{ kind:"event", name, payload }` — `pty:data`, `pty:exit`

Allow-listed calls: `sessionsList`, `sessionsLogsBulk`, `sessionLogs`,
`busRecent`, `sessionStop`, `sessionSendMessage`, `ptyOpen`/`Write`/`Resize`/
`Close`, `stateGet`/`stateSet`. The frame cannot call any Tauri command directly;
Goia refuses that by design (the frame is a remote loopback origin).

## What maps to what

- A Goia **session** → a node on the board. Positions, the camera and hand-drawn
  links are the plugin's own, saved as opaque state via `stateGet`/`stateSet`
  (Goia keeps `…/extensions/godesk/state.json` but never reads it).
- Each node shows the **polled mirror** (`sessionsLogsBulk`); the **focused** node
  attaches live (`ptyOpen` + `pty:data`) as an interactive xterm.
- **Lines**: hand-drawn links (dragged from a node's handle) plus detected links
  from Goia's inter-agent bus (`busRecent`).

## Known limits (inherited from Goia's engine, MVP)

- Live interactive attach is Claude-only in Goia today; other providers show the
  read-only mirror.
- No fleet-change event in Goia, so the board polls `sessionsList`.
- Attach has no replay buffer, so a just-focused node fills as new output arrives.
