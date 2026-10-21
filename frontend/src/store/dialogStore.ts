import { create } from "zustand";

export type ConfirmOptions = {
  title: string;
  message: string;
  confirmLabel: string;
  danger?: boolean;
};

type DialogState = {
  pending: (ConfirmOptions & { resolve: (ok: boolean) => void }) | null;
  open: (options: ConfirmOptions) => Promise<boolean>;
  close: (ok: boolean) => void;
};

/** In-app confirmation dialog; native `window.confirm` is unreliable in webviews. */
export const useDialogStore = create<DialogState>()((set, get) => ({
  pending: null,
  open(options) {
    get().pending?.resolve(false);
    return new Promise<boolean>((resolve) => set({ pending: { ...options, resolve } }));
  },
  close(ok) {
    get().pending?.resolve(ok);
    set({ pending: null });
  },
}));

export const confirmDialog = (options: ConfirmOptions) => useDialogStore.getState().open(options);
