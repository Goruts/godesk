import type { ReactNode } from "react";

/** 16×16 line icons drawn with the current text color. Decorative: label the control instead. */
function Icon({ size = 16, children }: { size?: number | undefined; children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      aria-hidden="true"
      focusable="false"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {children}
    </svg>
  );
}

export function PlusIcon({ size }: { size?: number | undefined }) {
  return (
    <Icon size={size}>
      <path d="M8 3.25v9.5M3.25 8h9.5" />
    </Icon>
  );
}

export function KnowledgeIcon({ size }: { size?: number | undefined }) {
  return (
    <Icon size={size}>
      <path d="M8 4.2C6.8 3.4 5.2 3.1 3.3 3.3 2.9 3.3 2.6 3.6 2.6 4v7.3c0 .4.4.7.8.6 1.7-.2 3.1.1 4.1.8 1-.7 2.4-1 4.1-.8.4 0 .8-.2.8-.6V4c0-.4-.3-.7-.7-.7-1.9-.2-3.5.1-4.7.9Z" />
      <path d="M8 4.2v8.5" />
    </Icon>
  );
}

export function WorkspacesIcon() {
  return (
    <Icon>
      <rect x="2.25" y="2.25" width="4.75" height="4.75" rx="1.25" />
      <rect x="9" y="2.25" width="4.75" height="4.75" rx="1.25" />
      <rect x="2.25" y="9" width="4.75" height="4.75" rx="1.25" />
      <rect x="9" y="9" width="4.75" height="4.75" rx="1.25" />
    </Icon>
  );
}

export function ChevronIcon() {
  return (
    <Icon size={12}>
      <path d="M4.5 9.75 8 6.25l3.5 3.5" />
    </Icon>
  );
}

export function LinkIcon() {
  return (
    <Icon>
      <path d="M6.75 9.25l2.5-2.5M7.75 4.25l.75-.75a2.65 2.65 0 0 1 3.75 3.75l-.75.75M8.25 11.75l-.75.75a2.65 2.65 0 0 1-3.75-3.75l.75-.75" />
    </Icon>
  );
}

export function FitIcon() {
  return (
    <Icon>
      <path d="M2.75 5.75v-3h3M10.25 2.75h3v3M13.25 10.25v3h-3M5.75 13.25h-3v-3" />
    </Icon>
  );
}

export function MoreIcon() {
  return (
    <Icon>
      <path d="M3.75 8h.01M8 8h.01M12.25 8h.01" strokeWidth="2.2" />
    </Icon>
  );
}

/** Two columns of dots: the "drag me" handle. */
export function GripIcon() {
  return (
    <Icon size={14}>
      <path d="M6 3.5h.01M10 3.5h.01M6 8h.01M10 8h.01M6 12.5h.01M10 12.5h.01" strokeWidth="2" />
    </Icon>
  );
}

export function PencilIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M10.75 2.75l2.5 2.5L6 12.5l-3.25.75L3.5 10z" />
    </Icon>
  );
}

export function DuplicateIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="5.75" y="5.75" width="7.5" height="7.5" rx="1.5" />
      <path d="M10.25 3.25a1.5 1.5 0 0 0-1.5-1h-4.5a1.5 1.5 0 0 0-1.5 1.5v4.5a1.5 1.5 0 0 0 1 1.5" />
    </Icon>
  );
}

export function CloseIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M4.25 4.25l7.5 7.5M11.75 4.25l-7.5 7.5" />
    </Icon>
  );
}

/** A single low line: collapse the terminal to its title bar. */
export function MinimizeIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M4 11.5h8" />
    </Icon>
  );
}

/** An empty square: grow the terminal to fill the view. */
export function MaximizeIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="4" y="4" width="8" height="8" rx="1.5" />
    </Icon>
  );
}

/** Two offset squares: shrink a maximized terminal back to its old size. */
export function RestoreIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="4" y="5.5" width="6.5" height="6.5" rx="1.5" />
      <path d="M6.5 5.5V4.5a1.5 1.5 0 0 1 1.5-1.5h3a1.5 1.5 0 0 1 1.5 1.5v3a1.5 1.5 0 0 1-1.5 1.5h-1" />
    </Icon>
  );
}

/** A window with an arrow into it: bring another program onto the canvas. */
export function AppsIcon() {
  return (
    <Icon>
      <rect x="2.25" y="3.25" width="11.5" height="9.5" rx="1.5" />
      <path d="M2.25 6h11.5M6.5 9.5h3.25M8.25 8l1.5 1.5-1.5 1.5" />
    </Icon>
  );
}

/** Angle brackets: open in a code editor. */
export function CodeIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M5.5 4.5 2 8l3.5 3.5M10.5 4.5 14 8l-3.5 3.5" />
    </Icon>
  );
}

/** A box with an arrow leaving it: give a window back to the desktop. */
export function PopOutIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M9.25 2.75h4v4M13.25 2.75 7.5 8.5M11.25 9.5v2.25a1.5 1.5 0 0 1-1.5 1.5h-5.5a1.5 1.5 0 0 1-1.5-1.5v-5.5a1.5 1.5 0 0 1 1.5-1.5H6.5" />
    </Icon>
  );
}
