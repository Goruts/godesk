import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { api } from "../lib/api";
import { useAppStore } from "../store/appStore";
import { focusTerminalView } from "./TerminalView";

/** How many of a path's trailing segments the list shows. */
const MAX_LIST = 50;

/** A folder path split for display: its own name, and the folders above it. */
function prettyPath(path: string): { name: string; parent: string } {
  const parts = path.split("/").filter(Boolean);
  return { name: parts[parts.length - 1] ?? path, parent: parts.slice(0, -1).join("/") };
}

/**
 * A zoxide-style jump: a quick search over the folders the terminals have
 * worked in (ranked by frecency in the backend), opening a new terminal in the
 * chosen one. Opened with the keyboard shortcut; keyboard-driven throughout.
 */
export function FolderPalette() {
  const open = useAppStore((s) => s.folderPalette);
  const setOpen = useAppStore((s) => s.setFolderPalette);
  const createInFolder = useAppStore((s) => s.createTerminalInFolder);
  const [query, setQuery] = useState("");
  const [dirs, setDirs] = useState<string[]>([]);
  const [sel, setSel] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  // Load matches as the query changes, lightly debounced, while open.
  useEffect(() => {
    if (!open) return;
    let active = true;
    const timer = window.setTimeout(() => {
      api
        .listDirs(query)
        .then((list) => {
          if (active) {
            setDirs(list.slice(0, MAX_LIST));
            setSel(0);
          }
        })
        .catch(() => undefined);
    }, 70);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [open, query]);

  // Start clean and focused each time it opens.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setSel(0);
    const frame = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [open]);

  // Keep the highlighted row in view.
  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [sel, dirs]);

  if (!open) return null;

  const choose = async (path: string | undefined) => {
    if (!path) return;
    setOpen(false);
    const terminal = await createInFolder(path);
    if (terminal) focusTerminalView(terminal.id);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((i) => (dirs.length ? (i + 1) % dirs.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((i) => (dirs.length ? (i - 1 + dirs.length) % dirs.length : 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      void choose(dirs[sel]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    }
  };

  return (
    <div
      className="palette-overlay"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) setOpen(false);
      }}
    >
      <div className="palette" role="dialog" aria-label="Open a folder in a new terminal">
        <input
          ref={inputRef}
          className="palette-input"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Jump to a folder — opens a terminal there"
          spellCheck={false}
          aria-label="Search folders"
        />
        <ul className="palette-list" role="listbox" ref={listRef}>
          {dirs.length === 0 ? (
            <li className="palette-empty">
              {query ? "No matching folders yet." : "No folders learned yet — work in a few terminals first."}
            </li>
          ) : (
            dirs.map((path, i) => {
              const { name, parent } = prettyPath(path);
              return (
                <li key={path}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={i === sel}
                    className={`palette-item${i === sel ? " is-selected" : ""}`}
                    onMouseEnter={() => setSel(i)}
                    onMouseDown={(e) => {
                      e.preventDefault();
                      void choose(path);
                    }}
                  >
                    <span className="palette-name">{name}</span>
                    <span className="palette-parent">{parent}</span>
                  </button>
                </li>
              );
            })
          )}
        </ul>
        <div className="palette-foot">
          <kbd>↑↓</kbd> move · <kbd>Enter</kbd> open terminal · <kbd>Esc</kbd> close
        </div>
      </div>
    </div>
  );
}
