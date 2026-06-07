// Themed wrapper around @radix-ui/react-dialog. No consumers in
// Step 12 — included for future destructive-confirmation flows
// (delete project, force-overwrite, etc.) and the eventual rename /
// settings panels.
//
// Composable like radix: caller supplies trigger + content shape.
// The wrapper here themes the overlay and content surfaces.

import * as RadixDialog from "@radix-ui/react-dialog";
import type { ReactNode } from "react";
import { clsx } from "clsx";
import "./dialog.css";

export const Dialog = RadixDialog.Root;
export const DialogTrigger = RadixDialog.Trigger;
export const DialogClose = RadixDialog.Close;
export const DialogTitle = RadixDialog.Title;
export const DialogDescription = RadixDialog.Description;

type ContentProps = {
  children: ReactNode;
  /** Extra className on the dialog surface. */
  className?: string;
};

export function DialogContent({ children, className }: ContentProps) {
  return (
    <RadixDialog.Portal>
      <RadixDialog.Overlay className="montage-dialog-overlay" />
      <RadixDialog.Content className={clsx("montage-dialog", className)}>
        {children}
      </RadixDialog.Content>
    </RadixDialog.Portal>
  );
}
