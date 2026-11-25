import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import { createPortal } from "react-dom";
import { CheckIcon, ChevronDownIcon } from "./Icons";

export interface SelectOption<T extends string = string> {
  value: T;
  label: string;
}

interface SelectProps<T extends string = string> {
  id?: string;
  value: T;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  ariaLabel?: string;
  className?: string;
  style?: CSSProperties;
  disabled?: boolean;
  size?: "default" | "sm" | "xs";
  align?: "left" | "right";
}

interface MenuCoords {
  top: number;
  left: number;
  minWidth: number;
  maxHeight: number;
  openUp: boolean;
}

export function Select<T extends string = string>({
  id,
  value,
  options,
  onChange,
  ariaLabel,
  className,
  style,
  disabled = false,
  size = "default",
  align = "left",
}: SelectProps<T>) {
  const [open, setOpen] = useState(false);
  const [highlightedIdx, setHighlightedIdx] = useState<number>(-1);
  const [coords, setCoords] = useState<MenuCoords | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  const selectedIndex = useMemo(
    () => options.findIndex((o) => o.value === value),
    [options, value]
  );
  const selectedOption = selectedIndex >= 0 ? options[selectedIndex] : options[0];

  const computeCoords = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const viewportW = window.innerWidth;
    const viewportH = window.innerHeight;
    const margin = 8;
    const gap = 5;
    const estimatedHeight = Math.min(options.length * 32 + 10, 280);
    const spaceBelow = viewportH - rect.bottom - margin;
    const spaceAbove = rect.top - margin;
    const openUp = spaceBelow < estimatedHeight && spaceAbove > spaceBelow;
    const maxHeight = Math.max(
      120,
      Math.min(280, (openUp ? spaceAbove : spaceBelow) - gap)
    );
    const minWidth = Math.max(rect.width, 128);
    const rawLeft = align === "right" ? rect.right - minWidth : rect.left;
    const left = Math.max(margin, Math.min(rawLeft, viewportW - minWidth - margin));
    const top = openUp ? rect.top - gap : rect.bottom + gap;
    setCoords({ top, left, minWidth, maxHeight, openUp });
  }, [align, options.length]);

  useLayoutEffect(() => {
    if (!open) return;
    computeCoords();
    setHighlightedIdx(selectedIndex >= 0 ? selectedIndex : 0);
  }, [open, computeCoords, selectedIndex]);

  useEffect(() => {
    if (!open) return;

    const onPointerDown = (e: MouseEvent) => {
      const target = e.target as Node | null;
      if (
        (triggerRef.current && triggerRef.current.contains(target)) ||
        (menuRef.current && menuRef.current.contains(target))
      ) {
        return;
      }
      setOpen(false);
    };

    const onScroll = (e: Event) => {
      if (menuRef.current && menuRef.current.contains(e.target as Node | null)) {
        return;
      }
      setOpen(false);
    };

    const onResize = () => computeCoords();
    const onWindowBlur = () => setOpen(false);

    document.addEventListener("mousedown", onPointerDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
    window.addEventListener("blur", onWindowBlur);
    return () => {
      document.removeEventListener("mousedown", onPointerDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("blur", onWindowBlur);
    };
  }, [open, computeCoords]);

  const commitSelection = (idx: number) => {
    const opt = options[idx];
    if (!opt) return;
    onChange(opt.value);
    setOpen(false);
    triggerRef.current?.focus();
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (disabled || options.length === 0) return;

    if (e.key === "Escape") {
      if (open) {
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
      }
      return;
    }

    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (!open) {
        setOpen(true);
      } else {
        setHighlightedIdx((prev) => (prev + 1) % options.length);
      }
      return;
    }

    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (!open) {
        setOpen(true);
      } else {
        setHighlightedIdx((prev) => (prev <= 0 ? options.length - 1 : prev - 1));
      }
      return;
    }

    if (e.key === "Home" && open) {
      e.preventDefault();
      setHighlightedIdx(0);
      return;
    }

    if (e.key === "End" && open) {
      e.preventDefault();
      setHighlightedIdx(options.length - 1);
      return;
    }

    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (!open) {
        setOpen(true);
      } else if (highlightedIdx >= 0) {
        commitSelection(highlightedIdx);
      }
    }
  };

  const sizeClass =
    size === "sm"
      ? "custom-select-sm"
      : size === "xs"
        ? "custom-select-xs"
        : "";

  const menuStyle: CSSProperties | undefined = coords
    ? {
        position: "fixed",
        left: coords.left,
        minWidth: coords.minWidth,
        maxHeight: coords.maxHeight,
        ...(coords.openUp
          ? { bottom: window.innerHeight - coords.top }
          : { top: coords.top }),
      }
    : undefined;

  return (
    <>
      <button
        ref={triggerRef}
        id={id}
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        disabled={disabled}
        className={`custom-select-trigger ${sizeClass} ${open ? "open" : ""} ${className ?? ""}`}
        style={style}
        onClick={() => !disabled && setOpen((v) => !v)}
        onKeyDown={handleKeyDown}
      >
        <span className="custom-select-label">{selectedOption?.label ?? ""}</span>
        <ChevronDownIcon
          size={13}
          className={`custom-select-chevron ${open ? "open" : ""}`}
        />
      </button>

      {open &&
        coords &&
        typeof document !== "undefined" &&
        createPortal(
          <div
            ref={menuRef}
            role="listbox"
            aria-label={ariaLabel}
            className={`custom-select-menu ${coords.openUp ? "open-up" : ""}`}
            style={menuStyle}
          >
            {options.map((opt, idx) => {
              const selected = opt.value === value;
              const highlighted = idx === highlightedIdx;
              return (
                <button
                  key={opt.value}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  className={`custom-select-option ${selected ? "selected" : ""} ${
                    highlighted ? "highlighted" : ""
                  }`}
                  onMouseEnter={() => setHighlightedIdx(idx)}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    commitSelection(idx);
                  }}
                >
                  <span className="custom-select-option-label">{opt.label}</span>
                  {selected && <CheckIcon size={13} className="custom-select-check" />}
                </button>
              );
            })}
          </div>,
          document.body
        )}
    </>
  );
}
