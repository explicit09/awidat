import { cn } from "../ui/cn";
import mark from "./montage-icon.png";

type BrandMarkProps = {
  size?: number;
  className?: string;
};

export function BrandMark({ size = 20, className }: BrandMarkProps) {
  return (
    <img
      src={mark}
      alt=""
      width={size}
      height={size}
      className={cn(
        "block shrink-0 rounded-[7px] object-cover",
        "shadow-[0_0_0_1px_rgba(239,68,68,0.26),0_4px_16px_rgba(239,68,68,0.30)]",
        className,
      )}
      style={{ width: size, height: size }}
    />
  );
}
