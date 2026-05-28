// Settings modal open-state store.
//
// The Settings modal is opened from a few places:
//   - The ⚙ Settings icon in the IdentityRow chrome (top right).
//   - The ⌘, keyboard shortcut handled in App.tsx.
//
// Mirrors the small surface area of `useMode`: just a boolean +
// open/close/toggle. Persistence is intentionally NOT wired — the
// modal should always reopen closed on a fresh app launch.

import { create } from "zustand";

interface SettingsStore {
  isOpen: boolean;
  open: () => void;
  close: () => void;
  toggle: () => void;
}

export const useSettings = create<SettingsStore>((set) => ({
  isOpen: false,
  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen })),
}));
