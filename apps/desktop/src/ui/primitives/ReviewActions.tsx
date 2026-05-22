import { Check, PencilLine, Sparkles, X } from "lucide-react";
import { Button } from "./Button";
import { Inline } from "./Stack";
import { cn } from "../cn";

export type ReviewActionsProps = {
  onAccept?: () => void;
  onReject?: () => void;
  onRevise?: () => void;
  onRepair?: () => void;
  size?: "sm" | "md";
  showRepair?: boolean;
  className?: string;
  disabled?: boolean;
};

export function ReviewActions({
  onAccept,
  onReject,
  onRevise,
  onRepair,
  size = "md",
  showRepair = false,
  className,
  disabled,
}: ReviewActionsProps) {
  const btnSize = size === "sm" ? "sm" : "md";
  const iconCls = size === "sm" ? "h-3.5 w-3.5" : "h-4 w-4";
  return (
    <Inline gap="2" className={cn(className)}>
      <Button
        variant="accept"
        size={btnSize}
        onClick={onAccept}
        disabled={disabled}
        leadingIcon={<Check className={iconCls} aria-hidden />}
      >
        Accept
      </Button>
      <Button
        variant="reject"
        size={btnSize}
        onClick={onReject}
        disabled={disabled}
        leadingIcon={<X className={iconCls} aria-hidden />}
      >
        Reject
      </Button>
      <Button
        variant="revise"
        size={btnSize}
        onClick={onRevise}
        disabled={disabled}
        leadingIcon={<PencilLine className={iconCls} aria-hidden />}
      >
        Revise
      </Button>
      {showRepair ? (
        <Button
          variant="repair"
          size={btnSize}
          onClick={onRepair}
          disabled={disabled}
          leadingIcon={<Sparkles className={iconCls} aria-hidden />}
        >
          Agent repair
        </Button>
      ) : null}
    </Inline>
  );
}
