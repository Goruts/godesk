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

export function CloseIcon({ size = 14 }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M4.25 4.25l7.5 7.5M11.75 4.25l-7.5 7.5" />
    </Icon>
  );
}
