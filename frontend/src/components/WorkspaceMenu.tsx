import { useEffect, useRef, useState } from "react";
import type { Workspace } from "../bindings/Workspace";
import { activeWorkspace, useAppStore } from "../store/appStore";
import { confirmDialog } from "../store/dialogStore";
import { ChevronIcon, CloseIcon, PencilIcon, PlusIcon, WorkspacesIcon } from "./icons";
import { usePopover } from "./usePopover";

/** Same limit as the backend (`MAX_NAME_CHARS` in state.rs). */
const MAX_NAME_LENGTH = 64;

type RenameFieldProps = {
  name: string;
  /** `null` cancels; `viaKeyboard` is true for Enter and Escape. */
  onDone: (name: string | null, viaKeyboard: boolean) => void;
};

/**
 * Inline name editor: Enter or leaving the field saves, Escape cancels. It
 * finishes once, so a blur fired while it unmounts cannot save after Escape.
 */
function RenameField({ name, onDone }: RenameFieldProps) {
  const finished = useRef(false);
  const finish = (value: string | null, viaKeyboard: boolean) => {
    if (finished.current) return;
    finished.current = true;
    onDone(value, viaKeyboard);
  };

  return (
    <input
      className="ws-input"
      aria-label="Workspace name"
      defaultValue={name}
      autoFocus
      maxLength={MAX_NAME_LENGTH}
      onFocus={(e) => e.currentTarget.select()}
      onBlur={(e) => finish(e.currentTarget.value, false)}
      onKeyDown={(e) => {
        if (e.key === "Enter") finish(e.currentTarget.value, true);
        else if (e.key === "Escape") finish(null, true);
      }}
    />
  );
}

type WorkspaceItemProps = {
  workspace: Workspace;
  active: boolean;
  editing: boolean;
  canDelete: boolean;
  onEditingChange: (editing: boolean) => void;
  onPicked: () => void;
  onDeleted: () => void;
};

function WorkspaceItem({ workspace, active, editing, canDelete, onEditingChange, onPicked, onDeleted }: WorkspaceItemProps) {
  const selectRef = useRef<HTMLButtonElement>(null);
  const deleteRef = useRef<HTMLButtonElement>(null);
  const refocusAfterRename = useRef(false);
  const setActiveWorkspace = useAppStore((s) => s.setActiveWorkspace);
  const renameWorkspace = useAppStore((s) => s.renameWorkspace);
  const deleteWorkspace = useAppStore((s) => s.deleteWorkspace);
  const count = workspace.terminals.length;
  const terminalCount = `${count} terminal${count === 1 ? "" : "s"}`;

  const finishRename = (value: string | null, viaKeyboard: boolean) => {
    refocusAfterRename.current = viaKeyboard;
    onEditingChange(false);
    if (value !== null) void renameWorkspace(workspace.id, value);
  };

  // Hand the keyboard back to the item once the field is gone.
  useEffect(() => {
    if (editing || !refocusAfterRename.current) return;
    refocusAfterRename.current = false;
    selectRef.current?.focus();
  }, [editing]);

  const remove = async () => {
    const ok = await confirmDialog({
      title: `Delete “${workspace.name}”?`,
      message:
        count === 0
          ? "The workspace and its layout will be removed."
          : `Its ${terminalCount} will be closed and every program running in them stopped. This cannot be undone.`,
      confirmLabel: "Delete workspace",
      danger: true,
    });
    if (ok && (await deleteWorkspace(workspace.id))) onDeleted();
    else deleteRef.current?.focus();
  };

  return (
    <li className={`ws-item${active ? " is-active" : ""}`}>
      {editing ? (
        <RenameField name={workspace.name} onDone={finishRename} />
      ) : (
        <>
          <button
            ref={selectRef}
            type="button"
            className="ws-select"
            aria-current={active ? "true" : undefined}
            aria-label={`${workspace.name}, ${terminalCount}`}
            title={`${terminalCount}. F2 to rename.`}
            onClick={() => {
              void setActiveWorkspace(workspace.id);
              onPicked();
            }}
            onKeyDown={(e) => {
              if (e.key !== "F2") return;
              e.preventDefault();
              onEditingChange(true);
            }}
          >
            <span className="ws-dot" aria-hidden="true" />
            <span className="ws-name">{workspace.name}</span>
            <span className="ws-count" aria-hidden="true">
              {count}
            </span>
          </button>
          <div className="ws-actions">
            <button
              type="button"
              className="ws-action"
              aria-label={`Rename ${workspace.name}`}
              title="Rename workspace"
              onClick={() => onEditingChange(true)}
            >
              <PencilIcon size={13} />
            </button>
            <button
              ref={deleteRef}
              type="button"
              className="ws-action ws-action-danger"
              aria-label={`Delete ${workspace.name}`}
              disabled={!canDelete}
              title={canDelete ? "Delete workspace" : "The last workspace cannot be deleted"}
              onClick={() => void remove()}
            >
              <CloseIcon size={13} />
            </button>
          </div>
        </>
      )}
    </li>
  );
}

/**
 * Dock button naming the workspace on the canvas, with a panel to switch,
 * rename, create and delete workspaces. Switching only changes which
 * workspace the canvas shows: the terminals of the others keep running in the
 * backend, and their views re-attach (replaying the buffer) when their
 * workspace is shown again.
 */
export function WorkspaceMenu() {
  const ready = useAppStore((s) => s.ready);
  const workspaces = useAppStore((s) => s.workspaces);
  const activeWorkspaceId = useAppStore((s) => s.activeWorkspaceId);
  const activeName = useAppStore((s) => activeWorkspace(s)?.name ?? "Workspaces");
  const createWorkspace = useAppStore((s) => s.createWorkspace);
  const { open, toggle, close, buttonRef, panelRef } = usePopover();
  const [editingId, setEditingId] = useState<string | null>(null);
  const [focusRequest, setFocusRequest] = useState(0);
  const creating = useRef(false);
  const listRef = useRef<HTMLUListElement>(null);

  // After a delete, the keyboard moves to the workspace now shown.
  useEffect(() => {
    if (focusRequest === 0) return;
    listRef.current?.querySelector<HTMLElement>(".ws-item.is-active .ws-select")?.focus();
  }, [focusRequest]);

  // A rename left open must not come back when the panel reopens.
  useEffect(() => {
    if (!open) setEditingId(null);
  }, [open]);

  const create = async () => {
    // A double click must not create two workspaces.
    if (creating.current) return;
    creating.current = true;
    try {
      const ws = await createWorkspace();
      if (ws) setEditingId(ws.id);
    } finally {
      creating.current = false;
    }
  };

  return (
    <div className="popover-anchor">
      <button
        ref={buttonRef}
        type="button"
        className="dock-btn dock-btn-workspace"
        aria-label={`Workspace: ${activeName}`}
        aria-expanded={open}
        aria-controls="workspace-panel"
        data-tip={open ? undefined : "Workspaces"}
        onClick={toggle}
      >
        <WorkspacesIcon />
        <span className="dock-btn-text">{activeName}</span>
        <ChevronIcon />
      </button>
      {open && (
        <div ref={panelRef} id="workspace-panel" className="popover popover-start" role="dialog" aria-label="Workspaces">
          <h2 className="popover-title">Workspaces</h2>
          <ul ref={listRef} className="ws-list">
            {workspaces.map((ws) => (
              <WorkspaceItem
                key={ws.id}
                workspace={ws}
                active={ws.id === activeWorkspaceId}
                editing={ws.id === editingId}
                canDelete={workspaces.length > 1}
                onEditingChange={(editing) =>
                  setEditingId((current) => (editing ? ws.id : current === ws.id ? null : current))
                }
                onPicked={() => close(true)}
                onDeleted={() => setFocusRequest((n) => n + 1)}
              />
            ))}
          </ul>
          <button type="button" className="ws-new" disabled={!ready} onClick={() => void create()}>
            <PlusIcon size={14} />
            New workspace
          </button>
        </div>
      )}
    </div>
  );
}
